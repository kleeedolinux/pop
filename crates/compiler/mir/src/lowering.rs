//! HIR to canonical MIR lowering and portable effect/GC preparation.
//!
//! This module makes evaluation order, control flow, calls, failure edges,
//! roots, barriers, and safe points explicit. It consumes typed HIR and does
//! not perform source lookup or introduce backend-specific instructions.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    BindingId, BlockId, BorrowRegionId, BuiltinTypeId, CaptureId, ClassId, CleanupScopeId,
    CoroutineStateId, FieldId, FileId, FunctionId, IterationProtocolMethodId, LifetimeId, LocalId,
    MethodId, ResultCaseId, SourceSpan, SymbolId, SymbolIdentity, TextRange, TextSize, TypeId,
    ValueId, ValueParameterId,
};
use pop_hir::{
    HirAssignmentTarget, HirBubble, HirCallDispatch, HirCaptureMode, HirCaptureSource, HirClosure,
    HirCodecErrorMatchArm, HirDataSpecialization, HirDeclaration, HirDeclarationKind,
    HirErrorMatchArm, HirExpression, HirExpressionKind, HirFieldValue, HirFunction,
    HirGeneratedCodecEntry, HirGeneratedCodecEntryBody, HirGeneratedCodecMemberId,
    HirIterationProtocol, HirIterationSource, HirLocalBinding, HirMatchArm, HirResultMatchArm,
    HirStatement, HirStatementKind, HirTableEntry, hir_generic_call_instances,
    remap_hir_function_dispatches, specialize_hir_function,
};
use pop_runtime_interface::{
    ArrayElementMap, FfiAbiLayoutId, FfiCallbackLifetime, FfiCallbackThread, ObjectMap, ObjectSlot,
    RootSlot, SafePointId, StackMap, Trap, TrapKind,
};
use pop_target::{CAbiScalarKind, TargetSpec};
use pop_types::{
    FfiCIntegerKind, FloatKind, IntegerKind, IntegerValue, NumericConversionKind, PrimitiveType,
    ResultProvenance, SemanticType, TypeArena, TypedBinaryOperator, TypedCompoundOperator,
    TypedUnaryOperator, ffi_c_integer_kind, is_ffi_function_type_constructor,
    is_ffi_integer_abi_builtin_type, is_ffi_pointer_type_constructor,
};

use crate::ir::*;
use crate::verification::{
    instruction_operands, instruction_unwind_target, terminator_operands, terminator_targets,
    verify_mir_bubble,
};
use crate::{
    MirFfiCallbackAbi, MirFfiCallbackFingerprint, MirFfiCallbackSignature, MirFfiLayout,
    MirFfiLayoutCatalog, MirFfiLayoutField, MirFfiValueClass,
};

type OptionalFfiLayoutFingerprint<'a> = Option<&'a dyn Fn(&[u8]) -> String>;

/// Lowers a verified HIR Bubble to canonical MIR and verifies the result.
///
/// # Errors
///
/// Returns deterministic MIR invariant violations.
///
/// # Panics
///
/// Panics only if the toolchain's required native target is removed from the
/// accepted target inventory.
pub fn lower_hir_bubble(
    hir: &HirBubble,
    arena: &TypeArena,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    let target = TargetSpec::for_triple("x86_64-unknown-linux-gnu")
        .expect("the accepted native target is part of the target inventory");
    lower_hir_bubble_for_target_internal(hir, arena, &target, None)
}

/// Lowers HIR with artifact-owned SHA-256 identities for source-selected FFI
/// layouts.
///
/// # Errors
///
/// Returns deterministic MIR or FFI layout invariant violations.
///
/// # Panics
///
/// Panics only if the toolchain's required native target is removed from the
/// accepted target inventory.
pub fn lower_hir_bubble_with_fingerprint(
    hir: &HirBubble,
    arena: &TypeArena,
    fingerprint: impl Fn(&[u8]) -> String,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    let target = TargetSpec::for_triple("x86_64-unknown-linux-gnu")
        .expect("the accepted native target is part of the target inventory");
    lower_hir_bubble_for_target_internal(hir, arena, &target, Some(&fingerprint))
}

/// Lowers a verified HIR Bubble to canonical MIR for one exact target and
/// verifies the result.
///
/// # Errors
///
/// Returns deterministic MIR invariant violations.
pub fn lower_hir_bubble_for_target(
    hir: &HirBubble,
    arena: &TypeArena,
    target: &TargetSpec,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    lower_hir_bubble_for_target_internal(hir, arena, target, None)
}

/// Lowers HIR for one exact target with artifact-owned SHA-256 identities for
/// source-selected FFI layouts.
///
/// # Errors
///
/// Returns deterministic MIR or FFI layout invariant violations.
pub fn lower_hir_bubble_for_target_with_fingerprint(
    hir: &HirBubble,
    arena: &TypeArena,
    target: &TargetSpec,
    fingerprint: impl Fn(&[u8]) -> String,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    lower_hir_bubble_for_target_internal(hir, arena, target, Some(&fingerprint))
}

fn lower_hir_bubble_for_target_internal(
    hir: &HirBubble,
    arena: &TypeArena,
    target: &TargetSpec,
    fingerprint: OptionalFfiLayoutFingerprint<'_>,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    let all_declarations = specialization_declarations(hir);
    let all_methods = specialization_methods(hir);
    let reference_effects: BTreeMap<_, _> = hir
        .function_references()
        .iter()
        .map(|reference| {
            (
                reference.identity(),
                lower_effect_summary(reference.effects()),
            )
        })
        .collect();
    let function_references: Vec<_> = hir
        .function_references()
        .iter()
        .filter(|reference| reference.foreign_declaration().is_none())
        .map(|reference| MirFunctionReference {
            identity: reference.identity(),
            is_async: reference.is_async(),
            parameters: reference.parameters().to_vec(),
            results: reference.results().to_vec(),
            effects: lower_effect_summary(reference.effects()),
            lifetime_summary: reference.lifetime_summary().clone(),
        })
        .collect();
    let declarations: Vec<_> = all_declarations
        .iter()
        .copied()
        .filter(|declaration| match declaration.kind() {
            HirDeclarationKind::Record(record) => !arena.contains_type_parameter(record.type_id()),
            HirDeclarationKind::Union(union) => !arena.contains_type_parameter(union.type_id()),
            HirDeclarationKind::Class(class) => !arena.contains_type_parameter(class.type_id()),
            HirDeclarationKind::Interface(interface) => {
                !arena.contains_type_parameter(interface.type_id())
            }
            _ => true,
        })
        .filter_map(lower_declaration)
        .collect();
    let mut foreign_functions: Vec<_> = hir
        .foreign_functions()
        .iter()
        .map(|function| MirForeignFunction {
            function: function.function(),
            symbol: function.symbol(),
            parameters: function
                .parameters()
                .iter()
                .map(pop_hir::HirParameter::type_id)
                .collect(),
            results: function.results().to_vec(),
            parameter_layouts: Vec::new(),
            result_layouts: Vec::new(),
            effects: lower_effect_summary(function.effects()),
            declaration: function.declaration().clone(),
            reference_identity: None,
        })
        .collect();
    let mut next_foreign_symbol = hir
        .functions()
        .iter()
        .map(|function| function.symbol().raw())
        .chain(
            hir.foreign_functions()
                .iter()
                .map(|function| function.symbol().raw()),
        )
        .chain(
            hir.declarations()
                .iter()
                .map(|declaration| declaration.symbol().raw()),
        )
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut next_foreign_function = hir
        .functions()
        .iter()
        .map(|function| function.function().raw())
        .chain(
            hir.foreign_functions()
                .iter()
                .map(|function| function.function().raw()),
        )
        .chain(
            hir.methods()
                .iter()
                .map(|method| method.function().function().raw()),
        )
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut referenced_foreign_symbols = BTreeMap::new();
    for reference in hir.function_references() {
        let Some(source) = reference.foreign_declaration() else {
            continue;
        };
        let symbol = SymbolId::from_raw(next_foreign_symbol);
        next_foreign_symbol = next_foreign_symbol.saturating_add(1);
        let function = FunctionId::from_raw(next_foreign_function);
        next_foreign_function = next_foreign_function.saturating_add(1);
        let declaration = pop_types::ForeignFunctionDeclaration::new(
            symbol,
            source.external_symbol(),
            source.abi(),
            source.link_aliases().to_vec(),
            source.is_nonblocking(),
            source.span(),
        )
        .with_callback_pairs(source.callback_pairs().to_vec());
        referenced_foreign_symbols.insert(reference.identity(), symbol);
        foreign_functions.push(MirForeignFunction {
            function,
            symbol,
            parameters: reference.parameters().to_vec(),
            results: reference.results().to_vec(),
            parameter_layouts: Vec::new(),
            result_layouts: Vec::new(),
            effects: lower_effect_summary(reference.effects()),
            declaration,
            reference_identity: Some(reference.identity()),
        });
    }
    foreign_functions.sort_by_key(MirForeignFunction::symbol);
    let gc_schema = LoweringGcSchema::new(&declarations, arena);
    let (ffi_layouts, provisional_ffi_layouts) =
        source_ffi_layout_catalog(hir, arena, target, fingerprint)?;
    bind_foreign_layouts(&mut foreign_functions, &ffi_layouts, arena)?;
    let specialized_hir_functions = specialize_reachable_functions(hir, arena)?;
    let mut function_effects: BTreeMap<_, _> = specialized_hir_functions
        .iter()
        .map(|function| (function.symbol(), lower_effect_summary(function.effects())))
        .collect();
    function_effects.extend(
        foreign_functions
            .iter()
            .map(|function| (function.symbol(), function.effects())),
    );
    let method_effects: BTreeMap<_, _> = all_methods
        .iter()
        .copied()
        .filter(|method| method.function().type_parameters().is_empty())
        .map(|method| {
            (
                method.method(),
                lower_effect_summary(method.function().effects()),
            )
        })
        .collect();
    let builtin_interface_effects = collect_builtin_interface_effects();
    let mut nested_functions = Vec::new();
    let mut functions: Vec<_> = specialized_hir_functions
        .iter()
        .map(|function| {
            let (function, mut nested) = lower_function(
                function,
                arena,
                &gc_schema,
                &reference_effects,
                &function_effects,
                &method_effects,
                &builtin_interface_effects,
                &ffi_layouts,
            );
            nested_functions.append(&mut nested);
            function
        })
        .collect();
    functions.sort_by_key(MirFunction::symbol);
    let methods: Vec<MirMethod> = all_methods
        .iter()
        .copied()
        .filter(|method| method.function().type_parameters().is_empty())
        .map(|method| {
            let (function, mut nested) = lower_function(
                method.function(),
                arena,
                &gc_schema,
                &reference_effects,
                &function_effects,
                &method_effects,
                &builtin_interface_effects,
                &ffi_layouts,
            );
            nested_functions.append(&mut nested);
            MirMethod {
                method: method.method(),
                class: method.class(),
                function,
            }
        })
        .collect();
    nested_functions.sort_by_key(|function| (function.owner(), function.function()));
    let (generated_codec_adapters, mut generated_codec_functions) =
        lower_reachable_codec_adapters(hir, &functions, &methods, &nested_functions);
    functions.append(&mut generated_codec_functions);
    functions.sort_by_key(MirFunction::symbol);
    let mut mir = MirBubble {
        bubble: hir.bubble(),
        namespace: hir.namespace(),
        dependencies: hir.dependencies().to_vec(),
        declarations,
        functions,
        foreign_functions,
        methods,
        nested_functions,
        function_references,
        nominal_references: lower_nominal_reference_catalog(hir.nominal_references()),
        ffi_layouts,
        generated_codec_adapters,
    };
    bind_call_lifetime_contracts(&mut mir, arena);
    if provisional_ffi_layouts {
        if mir_uses_ffi_unsafe_memory(&mir) {
            return Err(vec![MirVerificationError::MissingFfiLayoutFingerprint]);
        }
        mir.ffi_layouts = MirFfiLayoutCatalog::empty(target);
    }
    rewrite_foreign_calls(&mut mir, &referenced_foreign_symbols);
    recompute_effects(&mut mir);
    while insert_gc_safe_points(&mut mir, arena) {
        // Backedge safe points make their containing function a GC safe point.
        // Recompute the transitive call effects before deciding which callers
        // also require a safe point immediately before the call.
        recompute_effects(&mut mir);
    }
    populate_foreign_call_roots(&mut mir);
    seal_effects(&mut mir);
    verify_mir_bubble(&mir, arena)?;
    Ok(mir)
}

fn lower_reachable_codec_adapters(
    hir: &HirBubble,
    functions: &[MirFunction],
    methods: &[MirMethod],
    nested_functions: &[MirNestedFunction],
) -> (Vec<MirGeneratedCodecAdapter>, Vec<MirFunction>) {
    let mut reachable = BTreeSet::new();
    let mut collect = |blocks: &[MirBlock]| {
        for block in blocks {
            for instruction in &block.instructions {
                match instruction.kind() {
                    MirInstructionKind::GeneratedCodecSchema(adapter)
                    | MirInstructionKind::CodecEncode { adapter, .. }
                    | MirInstructionKind::CodecDecode { adapter, .. } => {
                        reachable.insert(*adapter);
                    }
                    _ => {}
                }
            }
        }
    };
    for function in functions {
        collect(&function.blocks);
    }
    for method in methods {
        collect(&method.function.blocks);
    }
    for nested in nested_functions {
        collect(&nested.blocks);
    }
    let adapters = hir
        .generated_codec_adapters()
        .iter()
        .filter(|adapter| reachable.contains(&adapter.symbol()))
        .map(|adapter| MirGeneratedCodecAdapter {
            symbol: adapter.symbol(),
            target: adapter.target(),
            module: adapter.module(),
            visibility: adapter.visibility(),
            name: adapter.name().to_owned(),
            target_name: adapter.target_name().to_owned(),
            target_type: adapter.target_type(),
            schema_type: adapter.schema_type(),
            schema_version: adapter.schema_version(),
            projection_sha256: adapter.projection_sha256().to_owned(),
            members: adapter
                .members()
                .iter()
                .map(|member| MirGeneratedCodecMember {
                    ordinal: member.ordinal(),
                    name: member.name().to_owned(),
                    member: match member.member() {
                        HirGeneratedCodecMemberId::Field(field) => {
                            MirGeneratedCodecMemberId::Field(field)
                        }
                        HirGeneratedCodecMemberId::EnumCase(case) => {
                            MirGeneratedCodecMemberId::EnumCase(case)
                        }
                        HirGeneratedCodecMemberId::UnionCase(case) => {
                            MirGeneratedCodecMemberId::UnionCase(case)
                        }
                    },
                    types: member.types().to_vec(),
                    discriminant: member.discriminant(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut functions = hir
        .generated_codec_adapters()
        .iter()
        .filter(|adapter| reachable.contains(&adapter.symbol()))
        .flat_map(|adapter| {
            [
                lower_generated_codec_entry(adapter.encode_entry()),
                lower_generated_codec_entry(adapter.decode_entry()),
            ]
        })
        .collect::<Vec<_>>();
    functions.sort_by_key(MirFunction::symbol);
    (adapters, functions)
}

fn lower_generated_codec_entry(entry: &HirGeneratedCodecEntry) -> MirFunction {
    let span = entry.provenance().attachment();
    let arguments = entry
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, type_id)| MirBlockArgument {
            value: ValueId::from_raw(u32::try_from(index).unwrap_or(u32::MAX)),
            type_id: *type_id,
            span,
        })
        .collect::<Vec<_>>();
    let result_value =
        ValueId::from_raw(u32::try_from(entry.parameters().len()).unwrap_or(u32::MAX));
    let kind = match entry.body() {
        HirGeneratedCodecEntryBody::CodecEncode { adapter } => MirInstructionKind::CodecEncode {
            adapter,
            value: arguments[0].value(),
            writer: arguments[1].value(),
            result: BuiltinTypeId::from_raw(100),
            success: ResultCaseId::from_raw(0),
            failure: ResultCaseId::from_raw(1),
        },
        HirGeneratedCodecEntryBody::CodecDecode { adapter } => MirInstructionKind::CodecDecode {
            adapter,
            reader: arguments[0].value(),
            result: BuiltinTypeId::from_raw(100),
            success: ResultCaseId::from_raw(0),
            failure: ResultCaseId::from_raw(1),
        },
    };
    let instruction = MirInstruction {
        result: result_value,
        result_type: entry.results().first().copied(),
        effects: local_instruction_effects(&kind),
        effects_explicit: false,
        unwind: MirUnwindAction::Propagate,
        kind,
        span,
    };
    MirFunction {
        function: FunctionId::from_raw(entry.symbol().raw()),
        symbol: entry.symbol(),
        is_async: false,
        parameters: entry.parameters().to_vec(),
        parameter_view_borrows: vec![None; entry.parameters().len()],
        results: entry.results().to_vec(),
        lifetime_summary: pop_types::CallableLifetimeSummary::conservative(
            entry.parameters().len(),
            entry.results().len(),
        ),
        effects: lower_effect_summary(entry.effects()),
        effects_explicit: false,
        blocks: vec![MirBlock {
            block: BlockId::from_raw(0),
            cleanup: None,
            arguments,
            instructions: vec![instruction],
            terminator: MirTerminator::Return {
                values: vec![result_value],
            },
        }],
    }
}

fn bind_call_lifetime_contracts(mir: &mut MirBubble, arena: &TypeArena) {
    let local = mir
        .functions
        .iter()
        .map(|function| (function.symbol, function.lifetime_summary.clone()))
        .chain(mir.methods.iter().map(|method| {
            (
                method.function.symbol,
                method.function.lifetime_summary.clone(),
            )
        }))
        .collect::<BTreeMap<_, _>>();
    let referenced = mir
        .function_references
        .iter()
        .map(|function| (function.identity, function.lifetime_summary.clone()))
        .collect::<BTreeMap<_, _>>();
    for function in &mut mir.functions {
        bind_function_call_lifetimes(function, &local, &referenced, arena);
    }
    for method in &mut mir.methods {
        bind_function_call_lifetimes(&mut method.function, &local, &referenced, arena);
    }
}

fn bind_function_call_lifetimes(
    function: &mut MirFunction,
    local: &BTreeMap<SymbolId, pop_types::CallableLifetimeSummary>,
    referenced: &BTreeMap<SymbolIdentity, pop_types::CallableLifetimeSummary>,
    arena: &TypeArena,
) {
    let mut used = function
        .parameter_view_borrows
        .iter()
        .filter_map(|borrow| borrow.as_ref().map(|borrow| borrow.borrow_lifetime()))
        .chain(
            function
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| created_view_lifetime(instruction.kind())),
        )
        .collect::<BTreeSet<_>>();
    let mut next = 0_u32;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let result_kind = instruction
                .result_type
                .and_then(|type_id| arena.view_kind(type_id))
                .map(|kind| match kind {
                    pop_types::ViewKind::Bytes => MirViewKind::Bytes,
                    pop_types::ViewKind::Text => MirViewKind::Text,
                });
            let contract = match &mut instruction.kind {
                MirInstructionKind::CallDirect {
                    function,
                    lifetime_summary,
                    view_result,
                    ..
                } => local
                    .get(function)
                    .map(|summary| (lifetime_summary, view_result, summary.clone())),
                MirInstructionKind::CallReferenced {
                    function,
                    lifetime_summary,
                    view_result,
                    ..
                } => referenced
                    .get(function)
                    .map(|summary| (lifetime_summary, view_result, summary.clone())),
                _ => None,
            };
            let Some((lifetime_summary, view_result, exact)) = contract else {
                continue;
            };
            *lifetime_summary = exact.clone();
            *view_result = result_kind.and_then(|kind| {
                let ResultProvenance::ReturnsAlias(source) = exact.result_provenance().first()?
                else {
                    return None;
                };
                while used.contains(&LifetimeId::from_raw(next)) {
                    next = next.saturating_add(1);
                }
                let lifetime = LifetimeId::from_raw(next);
                next = next.saturating_add(1);
                used.insert(lifetime);
                Some(MirCallViewResult::new(kind, *source, lifetime))
            });
        }
    }
    for block in &mut function.blocks {
        block.instructions.retain(|instruction| {
            !matches!(instruction.kind(), MirInstructionKind::ViewEnd { .. })
        });
    }
    insert_view_end_frontiers(function);
}

fn lower_nominal_reference_catalog(
    catalog: &pop_hir::HirNominalReferenceCatalog,
) -> MirNominalReferenceCatalog {
    let interfaces = catalog
        .interfaces()
        .iter()
        .map(|reference| {
            MirInterfaceReference::new(
                MirNominalIdentity::new(
                    reference.identity().definition(),
                    reference.identity().arguments().to_vec(),
                    reference.identity().canonical().arguments().to_vec(),
                ),
                reference.interface(),
                reference.type_id(),
            )
        })
        .collect();
    let classes = catalog
        .classes()
        .iter()
        .map(|reference| {
            MirClassReference::new(
                MirNominalIdentity::new(
                    reference.identity().definition(),
                    reference.identity().arguments().to_vec(),
                    reference.identity().canonical().arguments().to_vec(),
                ),
                reference.class(),
                reference.type_id(),
                reference.is_open(),
                reference.base().zip(reference.base_type()),
                reference
                    .interfaces()
                    .iter()
                    .map(|interface| {
                        MirInterfaceReference::new(
                            MirNominalIdentity::new(
                                interface.identity().definition(),
                                interface.identity().arguments().to_vec(),
                                interface.identity().canonical().arguments().to_vec(),
                            ),
                            interface.interface(),
                            interface.type_id(),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    MirNominalReferenceCatalog::new(interfaces, classes)
}

fn source_ffi_layout_catalog(
    hir: &HirBubble,
    arena: &TypeArena,
    target: &TargetSpec,
    fingerprint: OptionalFfiLayoutFingerprint<'_>,
) -> Result<(MirFfiLayoutCatalog, bool), Vec<MirVerificationError>> {
    let mut buffer_elements = BTreeSet::new();
    let mut pointer_elements = BTreeSet::new();
    for semantic in
        (0..arena.len()).filter_map(|raw| arena.get(TypeId::from_raw(u32::try_from(raw).ok()?)))
    {
        match semantic {
            SemanticType::Builtin {
                definition,
                arguments,
            } if *definition == pop_types::FFI_BUFFER_TYPE_ID && arguments.len() == 1 => {
                buffer_elements.insert(arguments[0]);
            }
            SemanticType::Builtin {
                definition,
                arguments,
            } if pop_types::is_ffi_pointer_type_constructor(*definition)
                && arguments.len() == 1 =>
            {
                pointer_elements.insert(arguments[0]);
            }
            _ => {}
        }
    }
    let mut foreign_record_elements = BTreeMap::new();
    for (abi, type_id) in hir.foreign_functions().iter().flat_map(|function| {
        function
            .parameters()
            .iter()
            .map(pop_hir::HirParameter::type_id)
            .chain(function.results().iter().copied())
            .map(move |type_id| (function.declaration().abi(), type_id))
    }) {
        if matches!(arena.get(type_id), Some(SemanticType::Record(_))) {
            foreign_record_elements.insert((type_id, foreign_abi_key(abi)), abi);
        }
    }
    for (abi, type_id) in hir.function_references().iter().flat_map(|reference| {
        reference
            .foreign_declaration()
            .into_iter()
            .flat_map(move |declaration| {
                reference
                    .parameters()
                    .iter()
                    .chain(reference.results())
                    .copied()
                    .map(move |type_id| (declaration.abi(), type_id))
            })
    }) {
        if matches!(arena.get(type_id), Some(SemanticType::Record(_))) {
            foreign_record_elements.insert((type_id, foreign_abi_key(abi)), abi);
        }
    }
    for function in hir.foreign_functions() {
        collect_callback_record_elements(
            &function
                .parameters()
                .iter()
                .map(pop_hir::HirParameter::type_id)
                .collect::<Vec<_>>(),
            function.declaration(),
            arena,
            &mut foreign_record_elements,
        );
    }
    for reference in hir.function_references() {
        if let Some(declaration) = reference.foreign_declaration() {
            collect_callback_record_elements(
                reference.parameters(),
                declaration,
                arena,
                &mut foreign_record_elements,
            );
        }
    }
    if (!buffer_elements.is_empty()
        || !foreign_record_elements.is_empty()
        || hir
            .reference_ffi_layout_catalog()
            .is_some_and(|catalog| !catalog.entries().is_empty()))
        && fingerprint.is_none()
    {
        return Err(vec![MirVerificationError::MissingFfiLayoutFingerprint]);
    }
    let provisional = fingerprint.is_none() && !pointer_elements.is_empty();
    let mut elements = BTreeMap::new();
    for element in buffer_elements.into_iter().chain(pointer_elements) {
        elements.insert(
            (element, foreign_abi_key(pop_types::ForeignAbi::C)),
            pop_types::ForeignAbi::C,
        );
    }
    elements.extend(foreign_record_elements);
    let imported_catalog = hir.reference_ffi_layout_catalog();
    if imported_catalog.is_some_and(|catalog| catalog.target() != target.triple()) {
        return Err(vec![MirVerificationError::InvalidFfiLayoutCatalog]);
    }
    if let Some(imported) = imported_catalog {
        for entry in imported.entries() {
            elements.remove(&(entry.element(), foreign_abi_key(entry.abi())));
        }
    }
    if elements.is_empty() && imported_catalog.is_none_or(|catalog| catalog.entries().is_empty()) {
        return Ok((MirFfiLayoutCatalog::empty(target), false));
    }
    let trusted_records = hir
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            HirDeclarationKind::Record(record) if record.has_ffi_c_layout() => {
                Some((record.type_id(), record))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut entries = Vec::new();
    let mut by_type = BTreeMap::new();
    let mut next_id = 1;
    for ((element, _), abi) in elements {
        ensure_source_ffi_layout(
            element,
            abi,
            arena,
            target,
            &trusted_records,
            &mut entries,
            &mut by_type,
            &mut next_id,
        )
        .ok_or_else(|| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
    }
    let mut imported_identities = BTreeMap::new();
    if let Some(imported) = imported_catalog {
        for entry in imported.entries() {
            let original = FfiAbiLayoutId::new(entry.id())
                .ok_or_else(|| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
            let provisional = FfiAbiLayoutId::new(next_id)
                .ok_or_else(|| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
            next_id = next_id.saturating_add(1);
            if imported_identities.insert(original, provisional).is_some() {
                return Err(vec![MirVerificationError::InvalidFfiLayoutCatalog]);
            }
        }
        for entry in imported.entries() {
            let original = FfiAbiLayoutId::new(entry.id())
                .ok_or_else(|| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
            let provisional = imported_identities[&original];
            let value_class = match entry.value_class() {
                pop_hir::HirFfiValueClass::Integer => MirFfiValueClass::Integer,
                pop_hir::HirFfiValueClass::Float => MirFfiValueClass::Float,
                pop_hir::HirFfiValueClass::Pointer => MirFfiValueClass::Pointer,
                pop_hir::HirFfiValueClass::FunctionPointer => MirFfiValueClass::FunctionPointer,
                pop_hir::HirFfiValueClass::Handle => MirFfiValueClass::Handle,
                pop_hir::HirFfiValueClass::Record(fields) => MirFfiValueClass::Record(
                    fields
                        .iter()
                        .map(|field| {
                            let child = FfiAbiLayoutId::new(field.layout())
                                .and_then(|child| imported_identities.get(&child).copied())?;
                            Some(MirFfiLayoutField::new_named(
                                field.field(),
                                field.name(),
                                field.source_index(),
                                child,
                                field.offset(),
                            ))
                        })
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| vec![MirVerificationError::InvalidFfiLayoutCatalog])?,
                ),
            };
            entries.push(MirFfiLayout::new_for_abi(
                provisional,
                entry.element(),
                entry.size(),
                entry.alignment(),
                value_class,
                entry.abi(),
            ));
        }
    }
    let catalog = if let Some(fingerprint) = fingerprint {
        MirFfiLayoutCatalog::new(target, entries, arena, fingerprint)
    } else {
        let next = Cell::new(1_u64);
        MirFfiLayoutCatalog::new(target, entries, arena, |_| {
            let identity = next.get();
            next.set(identity.saturating_add(1));
            format!("{identity:016x}{:048x}", 0)
        })
    }
    .map_err(|_| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
    if let Some(imported) = imported_catalog {
        for expected in imported.entries() {
            let id = FfiAbiLayoutId::new(expected.id())
                .ok_or_else(|| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
            let actual = catalog
                .get(id)
                .ok_or_else(|| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
            if actual.element() != expected.element()
                || actual.size() != expected.size()
                || actual.alignment() != expected.alignment()
                || actual.abi() != expected.abi()
                || actual.descriptor() != expected.descriptor()
                || actual.fingerprint() != expected.fingerprint()
                || !imported_value_class_matches(actual.value_class(), expected.value_class())
            {
                return Err(vec![MirVerificationError::InvalidFfiLayoutCatalog]);
            }
        }
    }
    Ok((catalog, provisional))
}

fn collect_callback_record_elements(
    foreign_parameters: &[TypeId],
    declaration: &pop_types::ForeignFunctionDeclaration,
    arena: &TypeArena,
    output: &mut BTreeMap<(TypeId, u8), pop_types::ForeignAbi>,
) {
    for contract in declaration.callback_pairs() {
        let Some(callback_parameter) =
            foreign_parameters.get(usize::from(contract.callback_parameter_index()))
        else {
            continue;
        };
        let Some(SemanticType::Builtin {
            definition,
            arguments,
        }) = arena.get(*callback_parameter)
        else {
            continue;
        };
        if !is_ffi_function_type_constructor(*definition) || arguments.len() != 1 {
            continue;
        }
        let Some(SemanticType::Function {
            parameters,
            results,
            ..
        }) = arena.get(arguments[0])
        else {
            continue;
        };
        let abi = match contract.callback_abi() {
            pop_types::FfiCallbackAbi::C => pop_types::ForeignAbi::C,
            pop_types::FfiCallbackAbi::System => pop_types::ForeignAbi::System,
        };
        for type_id in parameters.iter().chain(results).copied() {
            if matches!(arena.get(type_id), Some(SemanticType::Record(_))) {
                output.insert((type_id, foreign_abi_key(abi)), abi);
            }
        }
    }
}

fn imported_value_class_matches(
    actual: &MirFfiValueClass,
    expected: &pop_hir::HirFfiValueClass,
) -> bool {
    match (actual, expected) {
        (MirFfiValueClass::Integer, pop_hir::HirFfiValueClass::Integer)
        | (MirFfiValueClass::Float, pop_hir::HirFfiValueClass::Float)
        | (MirFfiValueClass::Pointer, pop_hir::HirFfiValueClass::Pointer)
        | (MirFfiValueClass::FunctionPointer, pop_hir::HirFfiValueClass::FunctionPointer)
        | (MirFfiValueClass::Handle, pop_hir::HirFfiValueClass::Handle) => true,
        (MirFfiValueClass::Record(actual), pop_hir::HirFfiValueClass::Record(expected)) => {
            actual.len() == expected.len()
                && actual.iter().zip(expected).all(|(actual, expected)| {
                    actual.field() == expected.field()
                        && actual.name() == Some(expected.name())
                        && actual.source_index() == expected.source_index()
                        && actual.layout().raw() == expected.layout()
                        && actual.offset() == expected.offset()
                })
        }
        _ => false,
    }
}

fn mir_uses_ffi_unsafe_memory(mir: &MirBubble) -> bool {
    let functions = mir
        .functions
        .iter()
        .map(MirFunction::blocks)
        .chain(mir.methods.iter().map(|method| method.function.blocks()))
        .chain(mir.nested_functions.iter().map(MirNestedFunction::blocks));
    functions
        .flatten()
        .flat_map(MirBlock::instructions)
        .any(|instruction| {
            matches!(
                instruction.kind(),
                MirInstructionKind::FfiUnsafeLoad { .. }
                    | MirInstructionKind::FfiUnsafeStore { .. }
                    | MirInstructionKind::FfiUnsafeAdvance { .. }
                    | MirInstructionKind::FfiUnsafeCopy { .. }
                    | MirInstructionKind::FfiUnsafeAddress { .. }
                    | MirInstructionKind::FfiUnsafePointerFromAddress { .. }
            )
        })
}

fn ensure_source_ffi_layout(
    element: TypeId,
    abi: pop_types::ForeignAbi,
    arena: &TypeArena,
    target: &TargetSpec,
    trusted_records: &BTreeMap<TypeId, &pop_hir::HirRecordDeclaration>,
    entries: &mut Vec<MirFfiLayout>,
    by_type: &mut BTreeMap<(TypeId, u8), FfiAbiLayoutId>,
    next_id: &mut u64,
) -> Option<FfiAbiLayoutId> {
    let key = (element, foreign_abi_key(abi));
    if let Some(layout) = by_type.get(&key) {
        return Some(*layout);
    }
    let provisional = FfiAbiLayoutId::new(*next_id)?;
    *next_id = next_id.checked_add(1)?;
    by_type.insert(key, provisional);
    let (size, alignment, value_class) = match arena.get(element)? {
        SemanticType::Primitive(PrimitiveType::Integer(kind)) => {
            let size = u64::from(kind.bit_width()) / 8;
            (size, size, MirFfiValueClass::Integer)
        }
        SemanticType::Primitive(PrimitiveType::Float32) => (4, 4, MirFfiValueClass::Float),
        SemanticType::Primitive(PrimitiveType::Float64) => (8, 8, MirFfiValueClass::Float),
        SemanticType::Builtin { definition, .. }
            if is_ffi_integer_abi_builtin_type(*definition) =>
        {
            let layout = target
                .c_abi_scalar_layout(target_ffi_integer_kind(ffi_c_integer_kind(*definition)?))?;
            (layout.size(), layout.alignment(), MirFfiValueClass::Integer)
        }
        SemanticType::Builtin { definition, .. }
            if is_ffi_pointer_type_constructor(*definition) =>
        {
            let (size, alignment) = target.ffi_pointer_layout()?;
            (size, alignment, MirFfiValueClass::Pointer)
        }
        SemanticType::Builtin { definition, .. }
            if is_ffi_function_type_constructor(*definition) =>
        {
            let (size, alignment) = target.ffi_pointer_layout()?;
            (size, alignment, MirFfiValueClass::FunctionPointer)
        }
        SemanticType::Builtin { definition, .. }
            if *definition == pop_types::FFI_HANDLE_TYPE_ID =>
        {
            (8, 8, MirFfiValueClass::Handle)
        }
        SemanticType::Record(semantic_fields) => {
            let record = trusted_records.get(&element)?;
            if semantic_fields.len() != record.fields().len() || semantic_fields.is_empty() {
                return None;
            }
            let mut offset = 0_u64;
            let mut alignment = 1_u64;
            let mut fields = Vec::with_capacity(semantic_fields.len());
            for (source_index, field) in record.fields().iter().enumerate() {
                let field_type = semantic_fields
                    .iter()
                    .find_map(|(name, field_type)| (name == field.name()).then_some(*field_type))?;
                if field_type != field.field_type() {
                    return None;
                }
                let child = ensure_source_ffi_layout(
                    field_type,
                    abi,
                    arena,
                    target,
                    trusted_records,
                    entries,
                    by_type,
                    next_id,
                )?;
                let child_layout = entries.iter().find(|entry| entry.id() == child)?;
                alignment = alignment.max(child_layout.alignment());
                offset = align_ffi_offset(offset, child_layout.alignment())?;
                fields.push(MirFfiLayoutField::new_named(
                    field.field(),
                    field.name(),
                    u32::try_from(source_index).ok()?,
                    child,
                    offset,
                ));
                offset = offset.checked_add(child_layout.size())?;
            }
            let size = align_ffi_offset(offset, alignment)?;
            (size, alignment, MirFfiValueClass::Record(fields))
        }
        _ => return None,
    };
    entries.push(MirFfiLayout::new_for_abi(
        provisional,
        element,
        size,
        alignment,
        value_class,
        abi,
    ));
    Some(provisional)
}

const fn foreign_abi_key(abi: pop_types::ForeignAbi) -> u8 {
    match abi {
        pop_types::ForeignAbi::C => 0,
        pop_types::ForeignAbi::System => 1,
        pop_types::ForeignAbi::CUnwind => 2,
    }
}

fn bind_foreign_layouts(
    functions: &mut [MirForeignFunction],
    catalog: &MirFfiLayoutCatalog,
    arena: &TypeArena,
) -> Result<(), Vec<MirVerificationError>> {
    for function in functions {
        let abi = function.declaration().abi();
        function.parameter_layouts = function
            .parameters
            .iter()
            .map(|type_id| foreign_layout_binding(*type_id, abi, catalog, arena))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|()| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
        function.result_layouts = function
            .results
            .iter()
            .map(|type_id| foreign_layout_binding(*type_id, abi, catalog, arena))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|()| vec![MirVerificationError::InvalidFfiLayoutCatalog])?;
    }
    Ok(())
}

fn foreign_layout_binding(
    type_id: TypeId,
    abi: pop_types::ForeignAbi,
    catalog: &MirFfiLayoutCatalog,
    arena: &TypeArena,
) -> Result<Option<FfiAbiLayoutId>, ()> {
    if !matches!(arena.get(type_id), Some(SemanticType::Record(_))) {
        return Ok(None);
    }
    catalog
        .entries()
        .iter()
        .find(|entry| {
            entry.element() == type_id
                && entry.abi() == abi
                && matches!(entry.value_class(), MirFfiValueClass::Record(_))
        })
        .map(|entry| Some(entry.id()))
        .ok_or(())
}

fn align_ffi_offset(offset: u64, alignment: u64) -> Option<u64> {
    let mask = alignment.checked_sub(1)?;
    offset.checked_add(mask).map(|value| value & !mask)
}

const fn target_ffi_integer_kind(kind: FfiCIntegerKind) -> CAbiScalarKind {
    match kind {
        FfiCIntegerKind::Char => CAbiScalarKind::Char,
        FfiCIntegerKind::SignedChar => CAbiScalarKind::SignedChar,
        FfiCIntegerKind::UnsignedChar => CAbiScalarKind::UnsignedChar,
        FfiCIntegerKind::Short => CAbiScalarKind::Short,
        FfiCIntegerKind::UnsignedShort => CAbiScalarKind::UnsignedShort,
        FfiCIntegerKind::Int => CAbiScalarKind::Int,
        FfiCIntegerKind::UnsignedInt => CAbiScalarKind::UnsignedInt,
        FfiCIntegerKind::Long => CAbiScalarKind::Long,
        FfiCIntegerKind::UnsignedLong => CAbiScalarKind::UnsignedLong,
        FfiCIntegerKind::LongLong => CAbiScalarKind::LongLong,
        FfiCIntegerKind::UnsignedLongLong => CAbiScalarKind::UnsignedLongLong,
        FfiCIntegerKind::Size => CAbiScalarKind::Size,
        FfiCIntegerKind::PointerDifference => CAbiScalarKind::PointerDifference,
    }
}

fn rewrite_foreign_calls(
    bubble: &mut MirBubble,
    referenced_foreign_symbols: &BTreeMap<SymbolIdentity, SymbolId>,
) {
    let foreign = bubble
        .foreign_functions
        .iter()
        .map(MirForeignFunction::symbol)
        .collect::<BTreeSet<_>>();
    for instruction in bubble
        .functions
        .iter_mut()
        .flat_map(|function| &mut function.blocks)
        .chain(
            bubble
                .methods
                .iter_mut()
                .flat_map(|method| &mut method.function.blocks),
        )
        .chain(
            bubble
                .nested_functions
                .iter_mut()
                .flat_map(|nested| &mut nested.blocks),
        )
        .flat_map(|block| &mut block.instructions)
    {
        let (function, arguments, declared_effects, unwind) = match instruction.kind.clone() {
            MirInstructionKind::CallDirect {
                function,
                arguments,
                declared_effects,
                unwind,
                ..
            } if foreign.contains(&function) => (function, arguments, declared_effects, unwind),
            MirInstructionKind::CallReferenced {
                function,
                arguments,
                declared_effects,
                unwind,
                ..
            } if referenced_foreign_symbols.contains_key(&function) => (
                referenced_foreign_symbols[&function],
                arguments,
                declared_effects,
                unwind,
            ),
            _ => continue,
        };
        instruction.kind = MirInstructionKind::CallForeign {
            function,
            arguments,
            safe_point: SafePointId::new(0),
            roots: Vec::new(),
            declared_effects,
            unwind,
        };
    }
}

fn populate_foreign_call_roots(bubble: &mut MirBubble) {
    for block in bubble
        .functions
        .iter_mut()
        .flat_map(|function| &mut function.blocks)
        .chain(
            bubble
                .methods
                .iter_mut()
                .flat_map(|method| &mut method.function.blocks),
        )
        .chain(
            bubble
                .nested_functions
                .iter_mut()
                .flat_map(|nested| &mut nested.blocks),
        )
    {
        let mut previous_safe_point = None;
        for instruction in &mut block.instructions {
            match &mut instruction.kind {
                MirInstructionKind::GcSafePoint {
                    safe_point, roots, ..
                } => previous_safe_point = Some((*safe_point, roots.clone())),
                MirInstructionKind::CallForeign {
                    safe_point, roots, ..
                } => {
                    if let Some((previous, previous_roots)) = previous_safe_point.take() {
                        *safe_point = previous;
                        *roots = previous_roots;
                    }
                }
                _ => previous_safe_point = None,
            }
        }
    }
}

fn specialization_declarations(hir: &HirBubble) -> Vec<&pop_hir::HirDeclaration> {
    let mut declarations = BTreeMap::new();
    for declaration in hir.declarations().iter().chain(
        hir.function_references()
            .iter()
            .filter_map(|reference| reference.specialization_capsule())
            .flat_map(|capsule| capsule.declarations()),
    ) {
        let key = match declaration.kind() {
            HirDeclarationKind::Class(class) => (class.class().raw(), class.type_id().raw()),
            _ => (u32::MAX, declaration.symbol().raw()),
        };
        declarations.entry(key).or_insert(declaration);
    }
    let mut declarations = declarations.into_values().collect::<Vec<_>>();
    declarations.sort_by_key(|declaration| declaration.symbol());
    declarations
}

fn specialization_methods(hir: &HirBubble) -> Vec<&pop_hir::HirMethod> {
    let mut methods = BTreeMap::new();
    for method in hir.methods().iter().chain(
        hir.function_references()
            .iter()
            .filter_map(|reference| reference.specialization_capsule())
            .flat_map(|capsule| capsule.methods()),
    ) {
        methods.entry(method.method()).or_insert(method);
    }
    methods.into_values().collect()
}

fn specialize_reachable_functions(
    hir: &HirBubble,
    arena: &TypeArena,
) -> Result<Vec<HirFunction>, Vec<MirVerificationError>> {
    let all_declarations = specialization_declarations(hir);
    let all_methods = specialization_methods(hir);
    let reference_templates: BTreeMap<_, _> = hir
        .function_references()
        .iter()
        .filter_map(|reference| {
            reference
                .specialization_capsule()
                .map(|capsule| (capsule.root(), capsule.root_symbol()))
        })
        .collect();
    let local_functions = hir
        .functions()
        .iter()
        .map(|function| {
            remap_hir_function_dispatches(function, &BTreeMap::new(), &reference_templates)
        })
        .collect::<Vec<_>>();
    let capsule_functions = hir
        .function_references()
        .iter()
        .filter_map(|reference| reference.specialization_capsule())
        .flat_map(|capsule| capsule.functions().iter().cloned())
        .collect::<Vec<_>>();
    let templates: BTreeMap<_, _> = local_functions
        .iter()
        .chain(&capsule_functions)
        .map(|function| (function.symbol(), function))
        .collect();
    let mut instances = BTreeMap::new();
    let data_symbols: BTreeMap<_, _> = all_declarations
        .iter()
        .copied()
        .filter_map(|declaration| match declaration.kind() {
            HirDeclarationKind::Record(record)
                if !arena.contains_type_parameter(record.type_id()) =>
            {
                Some((record.type_id(), declaration.symbol()))
            }
            HirDeclarationKind::Union(union) if !arena.contains_type_parameter(union.type_id()) => {
                Some((union.type_id(), declaration.symbol()))
            }
            _ => None,
        })
        .collect();
    let mut data_fields = BTreeMap::new();
    for template in all_declarations.iter().copied().filter(|declaration| {
        matches!(declaration.kind(), HirDeclarationKind::Record(record)
            if arena.contains_type_parameter(record.type_id()))
    }) {
        let HirDeclarationKind::Record(template_record) = template.kind() else {
            continue;
        };
        for instance in all_declarations.iter().copied().filter(|declaration| {
            declaration.module() == template.module()
                && declaration.name() == template.name()
                && matches!(declaration.kind(), HirDeclarationKind::Record(record)
                    if !arena.contains_type_parameter(record.type_id()))
        }) {
            let HirDeclarationKind::Record(instance_record) = instance.kind() else {
                continue;
            };
            for template_field in template_record.fields() {
                if let Some(instance_field) = instance_record
                    .fields()
                    .iter()
                    .find(|field| field.name() == template_field.name())
                {
                    data_fields.insert(
                        (instance_record.type_id(), template_field.field()),
                        instance_field.field(),
                    );
                }
            }
        }
    }
    let mut data_classes = BTreeMap::new();
    let mut data_methods = BTreeMap::new();
    for template in all_declarations.iter().copied().filter(|declaration| {
        matches!(declaration.kind(), HirDeclarationKind::Class(class)
            if arena.contains_type_parameter(class.type_id()))
    }) {
        let HirDeclarationKind::Class(template_class) = template.kind() else {
            continue;
        };
        for instance in all_declarations.iter().copied().filter(|declaration| {
            declaration.module() == template.module()
                && declaration.name() == template.name()
                && matches!(declaration.kind(), HirDeclarationKind::Class(class)
                    if !arena.contains_type_parameter(class.type_id()))
        }) {
            let HirDeclarationKind::Class(instance_class) = instance.kind() else {
                continue;
            };
            data_classes.insert(
                instance_class.type_id(),
                (instance.symbol(), instance_class.class()),
            );
            for template_field in template_class.fields() {
                if let Some(instance_field) = instance_class
                    .fields()
                    .iter()
                    .find(|field| field.name() == template_field.name())
                {
                    data_fields.insert(
                        (instance_class.type_id(), template_field.field()),
                        instance_field.field(),
                    );
                }
            }
            for template_method in template_class.methods() {
                if let Some(instance_method) = instance_class.methods().iter().find(|method| {
                    method.name() == template_method.name()
                        && method.dispatch() == template_method.dispatch()
                }) {
                    data_methods.insert(
                        (instance_class.type_id(), template_method.method()),
                        instance_method.method(),
                    );
                }
            }
        }
    }
    let mut interface_instances = BTreeMap::new();
    let mut concrete_interfaces = BTreeMap::new();
    for template in all_declarations.iter().copied().filter(|declaration| {
        matches!(declaration.kind(), HirDeclarationKind::Interface(interface)
            if arena.contains_type_parameter(interface.type_id()))
    }) {
        let HirDeclarationKind::Interface(template_interface) = template.kind() else {
            continue;
        };
        for instance in all_declarations.iter().copied().filter(|declaration| {
            declaration.module() == template.module()
                && declaration.name() == template.name()
                && matches!(declaration.kind(), HirDeclarationKind::Interface(interface)
                    if !arena.contains_type_parameter(interface.type_id()))
        }) {
            let HirDeclarationKind::Interface(instance_interface) = instance.kind() else {
                continue;
            };
            let methods = template_interface
                .methods()
                .iter()
                .filter_map(|template_method| {
                    instance_interface
                        .methods()
                        .iter()
                        .find(|method| method.slot() == template_method.slot())
                        .map(|method| (template_method.method(), method.method()))
                })
                .collect::<BTreeMap<_, _>>();
            for (template_method, concrete_method) in &methods {
                interface_instances.insert(
                    (
                        instance_interface.type_id(),
                        template_interface.interface(),
                        *template_method,
                    ),
                    (instance_interface.interface(), *concrete_method),
                );
            }
            concrete_interfaces.insert(
                instance_interface.interface(),
                (template_interface.interface(), methods),
            );
        }
    }
    for declaration in &all_declarations {
        let HirDeclarationKind::Class(class) = declaration.kind() else {
            continue;
        };
        if arena.contains_type_parameter(class.type_id()) {
            continue;
        }
        for implementation in class.interfaces() {
            let Some((template_interface, methods)) =
                concrete_interfaces.get(&implementation.interface())
            else {
                continue;
            };
            for (template_method, concrete_method) in methods {
                interface_instances.insert(
                    (class.type_id(), *template_interface, *template_method),
                    (implementation.interface(), *concrete_method),
                );
            }
        }
    }
    let data_instances = HirDataSpecialization::new(data_symbols, data_fields)
        .with_classes(data_classes, data_methods)
        .with_interfaces(interface_instances);
    let mut pending = BTreeSet::new();
    for function in local_functions
        .iter()
        .chain(capsule_functions.iter())
        .filter(|function| function.type_parameters().is_empty())
    {
        pending.extend(hir_generic_call_instances(function));
    }
    let mut next_symbol = templates
        .values()
        .copied()
        .map(HirFunction::symbol)
        .chain(
            all_declarations
                .iter()
                .map(|declaration| declaration.symbol()),
        )
        .chain(all_methods.iter().map(|method| method.function().symbol()))
        .map(SymbolId::raw)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    while let Some(key) = pending.pop_first() {
        if instances.contains_key(&key) {
            continue;
        }
        let symbol = SymbolId::from_raw(next_symbol);
        next_symbol = next_symbol.saturating_add(1);
        instances.insert(key.clone(), symbol);
        let Some(template) = templates.get(&key.0) else {
            return Err(vec![MirVerificationError::UnknownGenericTemplate(key.0)]);
        };
        if let Some(specialized) =
            specialize_hir_function(template, symbol, &key.1, &instances, &data_instances, arena)
        {
            pending.extend(hir_generic_call_instances(&specialized));
        } else {
            return Err(vec![MirVerificationError::InvalidGenericSpecialization(
                key.0,
            )]);
        }
        if instances.len() >= 4096 {
            return Err(vec![
                MirVerificationError::GenericSpecializationBudgetExceeded { limit: 4096 },
            ]);
        }
    }
    let mut specialized = Vec::new();
    for function in local_functions
        .iter()
        .chain(capsule_functions.iter())
        .filter(|function| function.type_parameters().is_empty())
    {
        specialized.push(
            specialize_hir_function(
                function,
                function.symbol(),
                &[],
                &instances,
                &data_instances,
                arena,
            )
            .ok_or_else(|| {
                vec![MirVerificationError::InvalidGenericSpecialization(
                    function.symbol(),
                )]
            })?,
        );
    }
    for ((source, arguments), symbol) in &instances {
        let template = templates
            .get(source)
            .ok_or_else(|| vec![MirVerificationError::UnknownGenericTemplate(*source)])?;
        specialized.push(
            specialize_hir_function(
                template,
                *symbol,
                arguments,
                &instances,
                &data_instances,
                arena,
            )
            .ok_or_else(|| vec![MirVerificationError::InvalidGenericSpecialization(*source)])?,
        );
    }
    specialized.sort_by_key(HirFunction::symbol);
    Ok(specialized)
}

fn seal_effects(bubble: &mut MirBubble) {
    for function in &mut bubble.functions {
        seal_function_effects(function);
    }
    for method in &mut bubble.methods {
        seal_function_effects(&mut method.function);
    }
    for function in &mut bubble.nested_functions {
        seal_nested_effects(function);
    }
}

fn seal_nested_effects(function: &mut MirNestedFunction) {
    function.effects_explicit = true;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            instruction.effects_explicit = true;
        }
    }
}

fn remove_inactive_view_ends(function: &mut MirFunction) {
    let entry = function.blocks().first().map(MirBlock::block);
    let initial = function
        .parameter_view_borrows()
        .iter()
        .filter_map(|borrow| borrow.as_ref().map(|borrow| borrow.borrow_lifetime()))
        .collect::<BTreeSet<_>>();
    let mut incoming = entry
        .map(|entry| (entry, initial))
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut pending = entry.into_iter().collect::<Vec<_>>();
    while let Some(block_id) = pending.pop() {
        let Some(block) = function
            .blocks()
            .iter()
            .find(|block| block.block() == block_id)
        else {
            continue;
        };
        let mut active = incoming.get(&block_id).cloned().unwrap_or_default();
        for instruction in block.instructions() {
            if let Some(lifetime) = created_view_lifetime(instruction.kind()) {
                active.insert(lifetime);
            } else if let MirInstructionKind::ViewEnd { borrow_lifetime } = instruction.kind() {
                active.remove(borrow_lifetime);
            }
            if let Some(target) = instruction_unwind_target(instruction) {
                merge_view_frontier_state(target, &active, &mut incoming, &mut pending);
            }
        }
        for target in terminator_targets(block.terminator()) {
            merge_view_frontier_state(target, &active, &mut incoming, &mut pending);
        }
    }
    for block in &mut function.blocks {
        let mut active = incoming.get(&block.block()).cloned().unwrap_or_default();
        block.instructions.retain(|instruction| {
            if let Some(lifetime) = created_view_lifetime(instruction.kind()) {
                active.insert(lifetime);
                true
            } else if let MirInstructionKind::ViewEnd { borrow_lifetime } = instruction.kind() {
                active.remove(borrow_lifetime)
            } else {
                true
            }
        });
    }
}

fn seal_function_effects(function: &mut MirFunction) {
    function.effects_explicit = true;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            instruction.effects_explicit = true;
        }
    }
}

fn lower_declaration(declaration: &HirDeclaration) -> Option<MirDeclaration> {
    let kind = match declaration.kind() {
        HirDeclarationKind::Record(record) => MirDeclarationKind::Record(MirRecordDeclaration {
            type_id: record.type_id(),
            fields: record
                .fields()
                .iter()
                .map(|field| MirField {
                    field: field.field(),
                    field_type: field.field_type(),
                })
                .collect(),
        }),
        HirDeclarationKind::Union(union) => MirDeclarationKind::Union(MirUnionDeclaration {
            type_id: union.type_id(),
            cases: union
                .cases()
                .iter()
                .map(|case| MirUnionCase {
                    case: case.case(),
                    parameters: case
                        .parameters()
                        .iter()
                        .map(pop_hir::HirNamedType::type_id)
                        .collect(),
                })
                .collect(),
        }),
        HirDeclarationKind::Error(error) => MirDeclarationKind::Error(MirErrorDeclaration {
            error: error.error(),
            type_id: error.type_id(),
            cases: error
                .cases()
                .iter()
                .map(|case| MirErrorCase {
                    case: case.case(),
                    parameters: case
                        .parameters()
                        .iter()
                        .map(pop_hir::HirNamedType::type_id)
                        .collect(),
                })
                .collect(),
        }),
        HirDeclarationKind::Enum(enumeration) => MirDeclarationKind::Enum(MirEnumDeclaration {
            type_id: enumeration.type_id(),
            cases: enumeration
                .cases()
                .iter()
                .map(|case| MirEnumCase {
                    case: case.case(),
                    discriminant: case.discriminant(),
                })
                .collect(),
        }),
        HirDeclarationKind::Class(class) => MirDeclarationKind::Class(MirClassDeclaration {
            definition: class.definition(),
            class: class.class(),
            type_id: class.type_id(),
            is_open: class.is_open(),
            base: None,
            fields: class
                .fields()
                .iter()
                .map(|field| MirField {
                    field: field.field(),
                    field_type: field.field_type(),
                })
                .collect(),
            methods: class
                .methods()
                .iter()
                .map(pop_hir::HirClassMethod::method)
                .collect(),
            interfaces: class
                .interfaces()
                .iter()
                .map(|implementation| MirInterfaceImplementation {
                    interface: implementation.interface(),
                    interface_type: implementation.interface_type(),
                    methods: implementation
                        .methods()
                        .iter()
                        .map(|method| MirInterfaceMethodImplementation {
                            interface_method: method.interface_method(),
                            slot: method.slot(),
                            class_method: method.class_method(),
                        })
                        .collect(),
                })
                .collect(),
            builtin_interfaces: class
                .builtin_interfaces()
                .iter()
                .map(|implementation| MirBuiltinInterfaceImplementation {
                    interface: implementation.interface(),
                    interface_type: implementation.interface_type(),
                    methods: implementation
                        .methods()
                        .iter()
                        .map(|method| MirBuiltinInterfaceMethodImplementation {
                            protocol_method: method.protocol_method(),
                            class_method: method.class_method(),
                        })
                        .collect(),
                })
                .collect(),
        }),
        HirDeclarationKind::Interface(interface) => {
            MirDeclarationKind::Interface(MirInterfaceDeclaration {
                interface: interface.interface(),
                type_id: interface.type_id(),
                methods: interface
                    .methods()
                    .iter()
                    .map(|method| MirInterfaceMethod {
                        method: method.method(),
                        slot: method.slot(),
                        parameters: method
                            .parameters()
                            .iter()
                            .map(pop_hir::HirNamedType::type_id)
                            .collect(),
                        results: method.results().to_vec(),
                        effects: lower_effect_summary(method.effects()),
                    })
                    .collect(),
            })
        }
        HirDeclarationKind::Attribute(_) => return None,
    };
    Some(MirDeclaration {
        symbol: declaration.symbol(),
        kind,
    })
}

fn lower_function(
    function: &HirFunction,
    arena: &TypeArena,
    gc_schema: &LoweringGcSchema,
    reference_effects: &BTreeMap<SymbolIdentity, MirEffectSummary>,
    function_effects: &BTreeMap<SymbolId, MirEffectSummary>,
    method_effects: &BTreeMap<MethodId, MirEffectSummary>,
    builtin_interface_effects: &BTreeMap<
        (BuiltinTypeId, IterationProtocolMethodId),
        MirEffectSummary,
    >,
    ffi_layouts: &MirFfiLayoutCatalog,
) -> (MirFunction, Vec<MirNestedFunction>) {
    let (mut lowered, nested) = FunctionBuilder::new(
        function,
        arena,
        gc_schema,
        reference_effects,
        function_effects,
        method_effects,
        builtin_interface_effects,
        ffi_layouts,
    )
    .lower();
    lowered.function = function.function();
    lowered.lifetime_summary = function.lifetime_summary().clone();
    insert_view_end_frontiers(&mut lowered);
    (lowered, nested)
}

fn insert_view_end_frontiers(function: &mut MirFunction) {
    let (_, _, live_out) = live_value_facts(function);
    let mut lifetime_views = BTreeMap::<pop_foundation::LifetimeId, BTreeSet<ValueId>>::new();
    let mut parents =
        BTreeMap::<pop_foundation::LifetimeId, Option<pop_foundation::LifetimeId>>::new();
    let mut view_lifetimes = BTreeMap::<ValueId, pop_foundation::LifetimeId>::new();
    let mut call_sources = Vec::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            let (lifetime, parent) = match instruction.kind() {
                MirInstructionKind::ViewCreate {
                    borrow_lifetime, ..
                } => (*borrow_lifetime, None),
                MirInstructionKind::ViewSlice {
                    borrow_lifetime,
                    parent_lifetime,
                    ..
                } => (*borrow_lifetime, Some(*parent_lifetime)),
                MirInstructionKind::CallDirect {
                    arguments,
                    view_result: Some(result),
                    ..
                }
                | MirInstructionKind::CallReferenced {
                    arguments,
                    view_result: Some(result),
                    ..
                } => {
                    if let Some(source) = arguments.get(usize::from(result.source_argument())) {
                        call_sources.push((result.borrow_lifetime(), *source));
                    }
                    (result.borrow_lifetime(), None)
                }
                _ => continue,
            };
            parents.insert(lifetime, parent);
            lifetime_views
                .entry(lifetime)
                .or_default()
                .insert(instruction.result());
            view_lifetimes.insert(instruction.result(), lifetime);
        }
    }
    if let Some(entry) = function.blocks().first() {
        for (argument, borrow) in entry
            .arguments()
            .iter()
            .zip(function.parameter_view_borrows())
        {
            if let Some(borrow) = borrow {
                lifetime_views
                    .entry(borrow.borrow_lifetime())
                    .or_default()
                    .insert(argument.value());
                parents.entry(borrow.borrow_lifetime()).or_insert(None);
                view_lifetimes.insert(argument.value(), borrow.borrow_lifetime());
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            if let MirTerminator::Branch { target, arguments } = block.terminator()
                && let Some(target) = function
                    .blocks()
                    .iter()
                    .find(|block| block.block() == *target)
            {
                for (source, target) in arguments.iter().zip(target.arguments()) {
                    if let Some(lifetime) = view_lifetimes.get(source).copied()
                        && view_lifetimes.insert(target.value(), lifetime) != Some(lifetime)
                    {
                        lifetime_views
                            .entry(lifetime)
                            .or_default()
                            .insert(target.value());
                        changed = true;
                    }
                }
            }
        }
    }
    for (lifetime, source) in call_sources {
        if let Some(parent) = view_lifetimes.get(&source).copied() {
            parents.insert(lifetime, Some(parent));
        }
    }

    let entry = function.blocks().first().map(MirBlock::block);
    let initial = function
        .parameter_view_borrows()
        .iter()
        .filter_map(|borrow| borrow.as_ref().map(|borrow| borrow.borrow_lifetime()))
        .collect::<BTreeSet<_>>();
    let mut incoming = entry
        .map(|entry| (entry, initial))
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut pending = entry.into_iter().collect::<Vec<_>>();
    while let Some(block_id) = pending.pop() {
        let Some(block) = function
            .blocks()
            .iter()
            .find(|block| block.block() == block_id)
        else {
            continue;
        };
        let mut active = incoming.get(&block_id).cloned().unwrap_or_default();
        for instruction in block.instructions() {
            if let Some(lifetime) = created_view_lifetime(instruction.kind()) {
                active.insert(lifetime);
            }
            if let Some(target) = instruction_unwind_target(instruction) {
                merge_view_frontier_state(target, &active, &mut incoming, &mut pending);
            }
        }
        for target in terminator_targets(block.terminator()) {
            merge_view_frontier_state(target, &active, &mut incoming, &mut pending);
        }
    }

    let mut next_value = function
        .blocks()
        .iter()
        .flat_map(|block| {
            block
                .arguments()
                .iter()
                .map(|argument| argument.value())
                .chain(block.instructions().iter().map(MirInstruction::result))
        })
        .map(ValueId::raw)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    for block in &mut function.blocks {
        let mut active = incoming.get(&block.block()).cloned().unwrap_or_default();
        for instruction in block.instructions() {
            if let Some(lifetime) = created_view_lifetime(instruction.kind()) {
                active.insert(lifetime);
            }
        }
        let exits = terminator_targets(block.terminator()).is_empty()
            || matches!(block.terminator(), MirTerminator::Suspend { .. });
        let live = live_out.get(&block.block()).cloned().unwrap_or_default();
        let mut ending = active
            .iter()
            .copied()
            .filter(|lifetime| {
                exits
                    || !lifetime_views
                        .get(lifetime)
                        .is_some_and(|views| views.iter().any(|view| live.contains(view)))
            })
            .collect::<Vec<_>>();
        ending.sort_by_key(|lifetime| std::cmp::Reverse(view_lifetime_depth(*lifetime, &parents)));
        for borrow_lifetime in ending {
            block.instructions.push(MirInstruction {
                result: ValueId::from_raw(next_value),
                result_type: None,
                kind: MirInstructionKind::ViewEnd { borrow_lifetime },
                effects: MirEffectSummary::empty(),
                effects_explicit: false,
                span: SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0))),
                unwind: MirUnwindAction::Propagate,
            });
            next_value = next_value.saturating_add(1);
        }
    }
    remove_inactive_view_ends(function);
}

fn created_view_lifetime(kind: &MirInstructionKind) -> Option<pop_foundation::LifetimeId> {
    match kind {
        MirInstructionKind::ViewCreate {
            borrow_lifetime, ..
        }
        | MirInstructionKind::ViewSlice {
            borrow_lifetime, ..
        } => Some(*borrow_lifetime),
        MirInstructionKind::CallDirect {
            view_result: Some(result),
            ..
        }
        | MirInstructionKind::CallReferenced {
            view_result: Some(result),
            ..
        } => Some(result.borrow_lifetime()),
        _ => None,
    }
}

fn merge_view_frontier_state(
    target: BlockId,
    active: &BTreeSet<pop_foundation::LifetimeId>,
    incoming: &mut BTreeMap<BlockId, BTreeSet<pop_foundation::LifetimeId>>,
    pending: &mut Vec<BlockId>,
) {
    let target_state = incoming.entry(target).or_default();
    let previous = target_state.len();
    target_state.extend(active.iter().copied());
    if target_state.len() != previous {
        pending.push(target);
    }
}

fn view_lifetime_depth(
    mut lifetime: pop_foundation::LifetimeId,
    parents: &BTreeMap<pop_foundation::LifetimeId, Option<pop_foundation::LifetimeId>>,
) -> usize {
    let mut depth = 0;
    while let Some(Some(parent)) = parents.get(&lifetime) {
        depth += 1;
        lifetime = *parent;
    }
    depth
}

fn collect_builtin_interface_effects()
-> BTreeMap<(BuiltinTypeId, IterationProtocolMethodId), MirEffectSummary> {
    let mut effects = BTreeMap::new();
    if let Some(protocol) = pop_types::embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol())
    {
        for (interface, method) in [
            (protocol.iterable(), protocol.iterator_method()),
            (protocol.iterator(), protocol.iterator_method()),
            (protocol.iterator(), protocol.next_method()),
        ] {
            if let Some(summary) = protocol.method_effects(interface, method) {
                effects.insert((interface, method), lower_effect_summary(summary));
            }
        }
    }
    effects
}

struct LoweringGcSchema {
    classes: BTreeMap<ClassId, ObjectMap>,
    fields: BTreeMap<FieldId, (ObjectSlot, TypeId)>,
}

impl LoweringGcSchema {
    fn new(declarations: &[MirDeclaration], arena: &TypeArena) -> Self {
        let mut classes = BTreeMap::new();
        let mut fields = BTreeMap::new();
        for declaration in declarations {
            let MirDeclarationKind::Class(class) = declaration.kind() else {
                continue;
            };
            let mut reference_slots = Vec::new();
            for (index, field) in class.fields().iter().enumerate() {
                let slot = ObjectSlot::new(u32::try_from(index).unwrap_or(u32::MAX));
                fields.insert(field.field(), (slot, field.field_type()));
                if is_managed_reference_type_id(field.field_type(), Some(arena)) {
                    reference_slots.push(slot);
                }
            }
            let slot_count = u32::try_from(class.fields().len()).unwrap_or(u32::MAX);
            let object_map = ObjectMap::new(slot_count, reference_slots)
                .expect("class field slots form a valid logical object map");
            classes.insert(class.class(), object_map);
        }
        Self { classes, fields }
    }
}

struct BuildingBlock {
    cleanup: Option<MirCleanupBlock>,
    arguments: Vec<MirBlockArgument>,
    instructions: Vec<MirInstruction>,
    terminator: MirTerminator,
}

#[derive(Clone, Copy)]
struct ActiveCleanup<'hir> {
    scope: CleanupScopeId,
    action: CleanupAction<'hir>,
}

#[derive(Clone, Copy)]
enum CleanupAction<'hir> {
    Statements(&'hir [HirStatement]),
    FfiBufferBorrow {
        buffer: ValueId,
        region: BorrowRegionId,
        span: SourceSpan,
    },
    FfiBytesBorrow {
        bytes: ValueId,
        region: BorrowRegionId,
        span: SourceSpan,
    },
    FfiCallback {
        callback: ValueId,
        region: BorrowRegionId,
        span: SourceSpan,
    },
}

#[derive(Clone)]
struct LiveState {
    parameters: Vec<ValueParameterId>,
    locals: Vec<LocalId>,
    specs: Vec<(TypeId, SourceSpan)>,
}

#[derive(Clone)]
struct LoopContext {
    break_target: BlockId,
    break_state: LiveState,
    continue_target: BlockId,
    continue_state: LiveState,
    cleanup_depth: usize,
}

enum LoweredAssignmentTarget {
    Local {
        local: LocalId,
        value_type: TypeId,
    },
    Capture {
        capture: CaptureId,
        value_type: TypeId,
    },
    Field {
        base: ValueId,
        field: FieldId,
        value_type: TypeId,
    },
    Array {
        array: ValueId,
        index: ValueId,
        array_type: TypeId,
        element_type: TypeId,
    },
    List {
        list: ValueId,
        index: ValueId,
        list_type: TypeId,
        element_type: TypeId,
    },
    Table {
        table: ValueId,
        key: ValueId,
        table_type: TypeId,
        value_type: TypeId,
    },
}

struct FunctionBuilder<'hir> {
    owner: SymbolId,
    is_async: bool,
    parameters_schema: Vec<TypeId>,
    results: Vec<TypeId>,
    body: &'hir [HirStatement],
    capture_schema: BTreeMap<CaptureId, MirCapture>,
    arena: &'hir TypeArena,
    gc_schema: &'hir LoweringGcSchema,
    reference_effects: &'hir BTreeMap<SymbolIdentity, MirEffectSummary>,
    function_effects: &'hir BTreeMap<SymbolId, MirEffectSummary>,
    method_effects: &'hir BTreeMap<MethodId, MirEffectSummary>,
    builtin_interface_effects:
        &'hir BTreeMap<(BuiltinTypeId, IterationProtocolMethodId), MirEffectSummary>,
    ffi_layouts: &'hir MirFfiLayoutCatalog,
    blocks: Vec<BuildingBlock>,
    current: BlockId,
    next_value: u32,
    parameters: BTreeMap<ValueParameterId, ValueId>,
    locals: BTreeMap<LocalId, ValueId>,
    parameter_cells: BTreeMap<ValueParameterId, ValueId>,
    local_cells: BTreeMap<LocalId, ValueId>,
    cell_parameters: BTreeSet<ValueParameterId>,
    cell_locals: BTreeSet<LocalId>,
    nested_functions: Vec<MirNestedFunction>,
    loop_stack: Vec<LoopContext>,
    active_cleanups: Vec<ActiveCleanup<'hir>>,
    next_cleanup_scope: u32,
    next_coroutine_state: u32,
    next_suspend_safe_point: u32,
    current_cleanup: Option<MirCleanupBlock>,
}

fn collect_cell_sources(
    statements: &[HirStatement],
) -> (BTreeSet<ValueParameterId>, BTreeSet<LocalId>) {
    let mut parameters = BTreeSet::new();
    let mut locals = BTreeSet::new();
    for statement in statements {
        visit_statement_closures(statement, &mut parameters, &mut locals);
    }
    (parameters, locals)
}

fn visit_statement_closures(
    statement: &HirStatement,
    parameters: &mut BTreeSet<ValueParameterId>,
    locals: &mut BTreeSet<LocalId>,
) {
    match statement.kind() {
        HirStatementKind::Local { initializer, .. } => {
            visit_expression_closures(initializer, parameters, locals);
        }
        HirStatementKind::MultipleLocal { value, .. } => {
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::LocalSet { value, .. }
        | HirStatementKind::ParameterSet { value, .. }
        | HirStatementKind::CaptureSet { value, .. }
        | HirStatementKind::Expression(value) => {
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::Return { values } => {
            for value in values {
                visit_expression_closures(value, parameters, locals);
            }
        }
        HirStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            visit_expression_closures(condition, parameters, locals);
            for nested in then_body.iter().chain(else_body) {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::OptionalIf {
            initializer,
            then_body,
            else_body,
            ..
        } => {
            visit_expression_closures(initializer, parameters, locals);
            for nested in then_body.iter().chain(else_body) {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::While { condition, body } => {
            visit_expression_closures(condition, parameters, locals);
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::OptionalWhile {
            initializer, body, ..
        } => {
            visit_expression_closures(initializer, parameters, locals);
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::RepeatUntil { body, condition } => {
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
            visit_expression_closures(condition, parameters, locals);
        }
        HirStatementKind::NumericFor {
            first,
            last,
            step,
            body,
            ..
        } => {
            visit_expression_closures(first, parameters, locals);
            visit_expression_closures(last, parameters, locals);
            visit_expression_closures(step, parameters, locals);
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::GeneralizedFor { iterable, body, .. } => {
            visit_expression_closures(iterable, parameters, locals);
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::Break | HirStatementKind::Continue => {}
        HirStatementKind::Match {
            scrutinee, arms, ..
        } => {
            visit_expression_closures(scrutinee, parameters, locals);
            for arm in arms {
                for nested in arm.body() {
                    visit_statement_closures(nested, parameters, locals);
                }
            }
        }
        HirStatementKind::ErrorMatch {
            scrutinee, arms, ..
        } => {
            visit_expression_closures(scrutinee, parameters, locals);
            for arm in arms {
                for nested in arm.body() {
                    visit_statement_closures(nested, parameters, locals);
                }
            }
        }
        HirStatementKind::ResultMatch {
            scrutinee, arms, ..
        } => {
            visit_expression_closures(scrutinee, parameters, locals);
            for arm in arms {
                for nested in arm.body() {
                    visit_statement_closures(nested, parameters, locals);
                }
            }
        }
        HirStatementKind::CodecErrorMatch { scrutinee, arms } => {
            visit_expression_closures(scrutinee, parameters, locals);
            for arm in arms {
                for nested in arm.body() {
                    visit_statement_closures(nested, parameters, locals);
                }
            }
        }
        HirStatementKind::Defer { body } | HirStatementKind::AsyncDefer { body } => {
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::FieldSet { base, value, .. } => {
            visit_expression_closures(base, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::CompoundFieldSet { base, value, .. } => {
            visit_expression_closures(base, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::ArraySet {
            array,
            index,
            value,
        } => {
            visit_expression_closures(array, parameters, locals);
            visit_expression_closures(index, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::ListSet { list, index, value } => {
            visit_expression_closures(list, parameters, locals);
            visit_expression_closures(index, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::TableSet { table, key, value } => {
            visit_expression_closures(table, parameters, locals);
            visit_expression_closures(key, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::CompoundArraySet {
            array,
            index,
            value,
            ..
        } => {
            visit_expression_closures(array, parameters, locals);
            visit_expression_closures(index, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::MultipleAssignment { targets, value } => {
            for target in targets {
                match target {
                    HirAssignmentTarget::Local { .. } | HirAssignmentTarget::Capture { .. } => {}
                    HirAssignmentTarget::Field { base, .. } => {
                        visit_expression_closures(base, parameters, locals);
                    }
                    HirAssignmentTarget::Array { array, index, .. } => {
                        visit_expression_closures(array, parameters, locals);
                        visit_expression_closures(index, parameters, locals);
                    }
                    HirAssignmentTarget::List { list, index, .. } => {
                        visit_expression_closures(list, parameters, locals);
                        visit_expression_closures(index, parameters, locals);
                    }
                    HirAssignmentTarget::Table { table, key, .. } => {
                        visit_expression_closures(table, parameters, locals);
                        visit_expression_closures(key, parameters, locals);
                    }
                }
            }
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::Call(call) => {
            for argument in call.arguments() {
                visit_expression_closures(argument, parameters, locals);
            }
        }
    }
}

fn contains_continue_for_current_loop(statements: &[HirStatement]) -> bool {
    statements.iter().any(|statement| match statement.kind() {
        HirStatementKind::Continue => true,
        HirStatementKind::If {
            then_body,
            else_body,
            ..
        } => {
            contains_continue_for_current_loop(then_body)
                || contains_continue_for_current_loop(else_body)
        }
        HirStatementKind::OptionalIf {
            then_body,
            else_body,
            ..
        } => {
            contains_continue_for_current_loop(then_body)
                || contains_continue_for_current_loop(else_body)
        }
        HirStatementKind::Match { arms, .. } => arms
            .iter()
            .any(|arm| contains_continue_for_current_loop(arm.body())),
        HirStatementKind::ErrorMatch { arms, .. } => arms
            .iter()
            .any(|arm| contains_continue_for_current_loop(arm.body())),
        HirStatementKind::ResultMatch { arms, .. } => arms
            .iter()
            .any(|arm| contains_continue_for_current_loop(arm.body())),
        HirStatementKind::CodecErrorMatch { arms, .. } => arms
            .iter()
            .any(|arm| contains_continue_for_current_loop(arm.body())),
        HirStatementKind::Defer { body } | HirStatementKind::AsyncDefer { body } => {
            contains_continue_for_current_loop(body)
        }
        HirStatementKind::While { .. }
        | HirStatementKind::OptionalWhile { .. }
        | HirStatementKind::RepeatUntil { .. }
        | HirStatementKind::NumericFor { .. }
        | HirStatementKind::GeneralizedFor { .. }
        | HirStatementKind::Local { .. }
        | HirStatementKind::MultipleLocal { .. }
        | HirStatementKind::LocalSet { .. }
        | HirStatementKind::ParameterSet { .. }
        | HirStatementKind::CaptureSet { .. }
        | HirStatementKind::Return { .. }
        | HirStatementKind::Break
        | HirStatementKind::FieldSet { .. }
        | HirStatementKind::CompoundFieldSet { .. }
        | HirStatementKind::ArraySet { .. }
        | HirStatementKind::ListSet { .. }
        | HirStatementKind::TableSet { .. }
        | HirStatementKind::CompoundArraySet { .. }
        | HirStatementKind::MultipleAssignment { .. }
        | HirStatementKind::Call(_)
        | HirStatementKind::Expression(_) => false,
    })
}

fn visit_expression_closures(
    expression: &HirExpression,
    parameters: &mut BTreeSet<ValueParameterId>,
    locals: &mut BTreeSet<LocalId>,
) {
    match expression.kind() {
        HirExpressionKind::Closure(closure) => {
            for capture in closure.captures() {
                if capture.mode() != HirCaptureMode::Cell {
                    continue;
                }
                match capture.source() {
                    HirCaptureSource::Local(local) => {
                        locals.insert(local);
                    }
                    HirCaptureSource::Parameter(parameter) => {
                        parameters.insert(parameter);
                    }
                    HirCaptureSource::Capture(_) => {}
                }
            }
        }
        HirExpressionKind::FfiBufferWithPointer {
            buffer: owner,
            body,
            ..
        }
        | HirExpressionKind::FfiBytesWithPin {
            bytes: owner, body, ..
        } => {
            visit_expression_closures(owner, parameters, locals);
            for capture in body.captures() {
                if capture.mode() != HirCaptureMode::Cell {
                    continue;
                }
                match capture.source() {
                    HirCaptureSource::Local(local) => {
                        locals.insert(local);
                    }
                    HirCaptureSource::Parameter(parameter) => {
                        parameters.insert(parameter);
                    }
                    HirCaptureSource::Capture(_) => {}
                }
            }
        }
        HirExpressionKind::FfiWithCallback { callback, body, .. } => {
            for closure in [callback, body] {
                for capture in closure.captures() {
                    if capture.mode() != HirCaptureMode::Cell {
                        continue;
                    }
                    match capture.source() {
                        HirCaptureSource::Local(local) => {
                            locals.insert(local);
                        }
                        HirCaptureSource::Parameter(parameter) => {
                            parameters.insert(parameter);
                        }
                        HirCaptureSource::Capture(_) => {}
                    }
                }
            }
        }
        HirExpressionKind::FfiCallbackOpen { callback, .. } => {
            for capture in callback.captures() {
                if capture.mode() != HirCaptureMode::Cell {
                    continue;
                }
                match capture.source() {
                    HirCaptureSource::Local(local) => {
                        locals.insert(local);
                    }
                    HirCaptureSource::Parameter(parameter) => {
                        parameters.insert(parameter);
                    }
                    HirCaptureSource::Capture(_) => {}
                }
            }
        }
        HirExpressionKind::FfiCallbackWithPair { callback, body, .. } => {
            visit_expression_closures(callback, parameters, locals);
            for capture in body.captures() {
                if capture.mode() != HirCaptureMode::Cell {
                    continue;
                }
                match capture.source() {
                    HirCaptureSource::Local(local) => {
                        locals.insert(local);
                    }
                    HirCaptureSource::Parameter(parameter) => {
                        parameters.insert(parameter);
                    }
                    HirCaptureSource::Capture(_) => {}
                }
            }
        }
        HirExpressionKind::FfiCallbackClose { callback, .. } => {
            visit_expression_closures(callback, parameters, locals);
        }
        HirExpressionKind::Field { base, .. }
        | HirExpressionKind::TupleGet { tuple: base, .. }
        | HirExpressionKind::InterfaceUpcast { value: base, .. }
        | HirExpressionKind::CheckedNominalCast { value: base, .. }
        | HirExpressionKind::NumericConvert { value: base, .. }
        | HirExpressionKind::StringFormat { value: base, .. } => {
            visit_expression_closures(base, parameters, locals);
        }
        HirExpressionKind::TableGet { table, key } => {
            visit_expression_closures(table, parameters, locals);
            visit_expression_closures(key, parameters, locals);
        }
        HirExpressionKind::ArrayGet { array, index }
        | HirExpressionKind::ArrayGetChecked { array, index }
        | HirExpressionKind::ListGet { list: array, index }
        | HirExpressionKind::ListGetChecked { list: array, index }
        | HirExpressionKind::Binary {
            left: array,
            right: index,
            ..
        }
        | HirExpressionKind::StringConcat {
            left: array,
            right: index,
        } => {
            visit_expression_closures(array, parameters, locals);
            visit_expression_closures(index, parameters, locals);
        }
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            visit_expression_closures(length, parameters, locals);
            visit_expression_closures(initial_value, parameters, locals);
        }
        HirExpressionKind::ArrayLength { array } => {
            visit_expression_closures(array, parameters, locals);
        }
        HirExpressionKind::ListCreate { capacity } => {
            if let Some(capacity) = capacity {
                visit_expression_closures(capacity, parameters, locals);
            }
        }
        HirExpressionKind::ListLength { list } => {
            visit_expression_closures(list, parameters, locals);
        }
        HirExpressionKind::ListAdd { list, value } => {
            visit_expression_closures(list, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirExpressionKind::RangeCreate { first, last, step } => {
            visit_expression_closures(first, parameters, locals);
            visit_expression_closures(last, parameters, locals);
            visit_expression_closures(step, parameters, locals);
        }
        HirExpressionKind::ArrayFill { array, value } => {
            visit_expression_closures(array, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirExpressionKind::Record { fields, .. }
        | HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                visit_expression_closures(field.value(), parameters, locals);
            }
        }
        HirExpressionKind::RecordUpdate { base, fields, .. } => {
            visit_expression_closures(base, parameters, locals);
            for field in fields {
                visit_expression_closures(field.value(), parameters, locals);
            }
        }
        HirExpressionKind::Array(elements)
        | HirExpressionKind::Tuple(elements)
        | HirExpressionKind::UnionCase {
            arguments: elements,
            ..
        }
        | HirExpressionKind::ResultCase {
            arguments: elements,
            ..
        }
        | HirExpressionKind::IterationCase {
            arguments: elements,
            ..
        }
        | HirExpressionKind::ErrorCase {
            arguments: elements,
            ..
        } => {
            for element in elements {
                visit_expression_closures(element, parameters, locals);
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                visit_expression_closures(entry.key(), parameters, locals);
                visit_expression_closures(entry.value(), parameters, locals);
            }
        }
        HirExpressionKind::Unary { operand, .. } => {
            visit_expression_closures(operand, parameters, locals);
        }
        HirExpressionKind::OptionalDefault { optional, fallback } => {
            visit_expression_closures(optional, parameters, locals);
            visit_expression_closures(fallback, parameters, locals);
        }
        HirExpressionKind::OptionalPropagate { optional, .. }
        | HirExpressionKind::OptionalNarrow { optional } => {
            visit_expression_closures(optional, parameters, locals);
        }
        HirExpressionKind::Await { task } => {
            visit_expression_closures(task, parameters, locals);
        }
        HirExpressionKind::TaskCancelToken { source }
        | HirExpressionKind::TaskCancel { source } => {
            visit_expression_closures(source, parameters, locals);
        }
        HirExpressionKind::FfiHandleOpen { value }
        | HirExpressionKind::FfiHandleGet { handle: value }
        | HirExpressionKind::FfiHandleClose { handle: value }
        | HirExpressionKind::FfiBufferOpen { length: value, .. }
        | HirExpressionKind::FfiBufferLength { buffer: value }
        | HirExpressionKind::FfiBufferClose { buffer: value }
        | HirExpressionKind::FfiPointerToOptional { pointer: value }
        | HirExpressionKind::FfiPointerReadOnly { pointer: value }
        | HirExpressionKind::FfiPointerIsPresent { pointer: value }
        | HirExpressionKind::FfiPointerRequire { pointer: value, .. } => {
            visit_expression_closures(value, parameters, locals);
        }
        HirExpressionKind::FfiUnsafeLoad { pointer, .. }
        | HirExpressionKind::FfiUnsafeAddress { pointer, .. }
        | HirExpressionKind::FfiUnsafePointerFromAddress {
            address: pointer, ..
        } => visit_expression_closures(pointer, parameters, locals),
        HirExpressionKind::FfiUnsafeStore { pointer, value, .. }
        | HirExpressionKind::FfiUnsafeAdvance {
            pointer,
            elements: value,
            ..
        } => {
            visit_expression_closures(pointer, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirExpressionKind::FfiUnsafeCopy {
            source,
            destination,
            count,
            ..
        } => {
            visit_expression_closures(source, parameters, locals);
            visit_expression_closures(destination, parameters, locals);
            visit_expression_closures(count, parameters, locals);
        }
        HirExpressionKind::FfiBufferRead { buffer, index } => {
            visit_expression_closures(buffer, parameters, locals);
            visit_expression_closures(index, parameters, locals);
        }
        HirExpressionKind::FfiBufferWrite {
            buffer,
            index,
            value,
        } => {
            visit_expression_closures(buffer, parameters, locals);
            visit_expression_closures(index, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirExpressionKind::TaskGroup { cancel, body } => {
            visit_expression_closures(cancel, parameters, locals);
            visit_expression_closures(body, parameters, locals);
        }
        HirExpressionKind::TaskStart { group, task } => {
            visit_expression_closures(group, parameters, locals);
            visit_expression_closures(task, parameters, locals);
        }
        HirExpressionKind::ResultPropagate { result, .. } => {
            visit_expression_closures(result, parameters, locals);
        }
        HirExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            visit_expression_closures(condition, parameters, locals);
            visit_expression_closures(when_true, parameters, locals);
            visit_expression_closures(when_false, parameters, locals);
        }
        HirExpressionKind::Call { arguments, .. } => {
            for argument in arguments {
                visit_expression_closures(argument, parameters, locals);
            }
        }
        HirExpressionKind::ViewCreate { lender, .. }
        | HirExpressionKind::ViewLength { view: lender, .. }
        | HirExpressionKind::ViewMaterialize { view: lender, .. } => {
            visit_expression_closures(lender, parameters, locals);
        }
        HirExpressionKind::ViewSlice {
            view,
            start,
            length,
            ..
        } => {
            visit_expression_closures(view, parameters, locals);
            visit_expression_closures(start, parameters, locals);
            visit_expression_closures(length, parameters, locals);
        }
        HirExpressionKind::ViewGetByte { view, index } => {
            visit_expression_closures(view, parameters, locals);
            visit_expression_closures(index, parameters, locals);
        }
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::CodecErrorCase(_)
        | HirExpressionKind::GeneratedCodecSchema(_)
        | HirExpressionKind::TaskCancellationSource
        | HirExpressionKind::FfiPointerNone { .. } => {}
        HirExpressionKind::EnumCase { .. } => {}
    }
}

impl<'hir> FunctionBuilder<'hir> {
    fn new(
        hir: &'hir HirFunction,
        arena: &'hir TypeArena,
        gc_schema: &'hir LoweringGcSchema,
        reference_effects: &'hir BTreeMap<SymbolIdentity, MirEffectSummary>,
        function_effects: &'hir BTreeMap<SymbolId, MirEffectSummary>,
        method_effects: &'hir BTreeMap<MethodId, MirEffectSummary>,
        builtin_interface_effects: &'hir BTreeMap<
            (BuiltinTypeId, IterationProtocolMethodId),
            MirEffectSummary,
        >,
        ffi_layouts: &'hir MirFfiLayoutCatalog,
    ) -> Self {
        let parameter_specs: Vec<_> = hir
            .parameters()
            .iter()
            .map(|parameter| (parameter.parameter(), parameter.type_id(), parameter.span()))
            .collect();
        Self::from_parts(
            hir.symbol(),
            hir.is_async(),
            parameter_specs,
            hir.results().to_vec(),
            hir.body(),
            BTreeMap::new(),
            arena,
            gc_schema,
            reference_effects,
            function_effects,
            method_effects,
            builtin_interface_effects,
            ffi_layouts,
        )
    }

    fn new_closure(
        owner: SymbolId,
        closure: &'hir HirClosure,
        arena: &'hir TypeArena,
        gc_schema: &'hir LoweringGcSchema,
        reference_effects: &'hir BTreeMap<SymbolIdentity, MirEffectSummary>,
        function_effects: &'hir BTreeMap<SymbolId, MirEffectSummary>,
        method_effects: &'hir BTreeMap<MethodId, MirEffectSummary>,
        builtin_interface_effects: &'hir BTreeMap<
            (BuiltinTypeId, IterationProtocolMethodId),
            MirEffectSummary,
        >,
        ffi_layouts: &'hir MirFfiLayoutCatalog,
    ) -> Self {
        let parameter_specs = closure
            .parameters()
            .iter()
            .map(|parameter| (parameter.parameter(), parameter.type_id(), parameter.span()))
            .collect();
        let capture_schema = closure
            .captures()
            .iter()
            .enumerate()
            .map(|(slot, capture)| {
                (
                    capture.capture(),
                    MirCapture {
                        capture: capture.capture(),
                        binding: capture.binding(),
                        slot: u32::try_from(slot).unwrap_or(u32::MAX),
                        type_id: capture.type_id(),
                        mode: match capture.mode() {
                            HirCaptureMode::Value => MirCaptureMode::Value,
                            HirCaptureMode::Cell => MirCaptureMode::Cell,
                        },
                    },
                )
            })
            .collect();
        Self::from_parts(
            owner,
            closure.is_async(),
            parameter_specs,
            closure.results().to_vec(),
            closure.body(),
            capture_schema,
            arena,
            gc_schema,
            reference_effects,
            function_effects,
            method_effects,
            builtin_interface_effects,
            ffi_layouts,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        owner: SymbolId,
        is_async: bool,
        parameter_specs: Vec<(ValueParameterId, TypeId, SourceSpan)>,
        results: Vec<TypeId>,
        body: &'hir [HirStatement],
        capture_schema: BTreeMap<CaptureId, MirCapture>,
        arena: &'hir TypeArena,
        gc_schema: &'hir LoweringGcSchema,
        reference_effects: &'hir BTreeMap<SymbolIdentity, MirEffectSummary>,
        function_effects: &'hir BTreeMap<SymbolId, MirEffectSummary>,
        method_effects: &'hir BTreeMap<MethodId, MirEffectSummary>,
        builtin_interface_effects: &'hir BTreeMap<
            (BuiltinTypeId, IterationProtocolMethodId),
            MirEffectSummary,
        >,
        ffi_layouts: &'hir MirFfiLayoutCatalog,
    ) -> Self {
        let mut arguments = Vec::new();
        let mut parameters = BTreeMap::new();
        for (parameter, type_id, span) in &parameter_specs {
            let value = ValueId::from_raw(parameter.raw());
            parameters.insert(*parameter, value);
            arguments.push(MirBlockArgument {
                value,
                type_id: *type_id,
                span: *span,
            });
        }
        let (cell_parameters, cell_locals) = collect_cell_sources(body);
        Self {
            owner,
            is_async,
            parameters_schema: parameter_specs
                .iter()
                .map(|(_, type_id, _)| *type_id)
                .collect(),
            results,
            body,
            capture_schema,
            arena,
            gc_schema,
            reference_effects,
            function_effects,
            method_effects,
            builtin_interface_effects,
            ffi_layouts,
            blocks: vec![BuildingBlock {
                cleanup: None,
                arguments,
                instructions: Vec::new(),
                terminator: MirTerminator::Missing,
            }],
            current: BlockId::from_raw(0),
            next_value: u32::try_from(parameter_specs.len()).unwrap_or(u32::MAX),
            parameters,
            locals: BTreeMap::new(),
            parameter_cells: BTreeMap::new(),
            local_cells: BTreeMap::new(),
            cell_parameters,
            cell_locals,
            nested_functions: Vec::new(),
            loop_stack: Vec::new(),
            active_cleanups: Vec::new(),
            next_cleanup_scope: 0,
            next_coroutine_state: 0,
            next_suspend_safe_point: 0,
            current_cleanup: None,
        }
    }

    fn lower(mut self) -> (MirFunction, Vec<MirNestedFunction>) {
        self.initialize_parameter_cells();
        self.lower_statements(self.body);
        for block in &mut self.blocks {
            if matches!(block.terminator, MirTerminator::Missing) {
                block.terminator = if self.results.is_empty() {
                    MirTerminator::Return { values: Vec::new() }
                } else {
                    MirTerminator::Unreachable
                };
            }
        }
        let blocks = self
            .blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| MirBlock {
                block: BlockId::from_raw(u32::try_from(index).unwrap_or(u32::MAX)),
                cleanup: block.cleanup,
                arguments: block.arguments,
                instructions: block.instructions,
                terminator: block.terminator,
            })
            .collect();
        let parameter_view_borrows = self
            .parameters_schema
            .iter()
            .enumerate()
            .map(|(index, type_id)| {
                let kind = match self.arena.get(*type_id) {
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if arguments.is_empty() && *definition == pop_types::BYTES_VIEW_TYPE_ID => {
                        MirViewKind::Bytes
                    }
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if arguments.is_empty() && *definition == pop_types::TEXT_VIEW_TYPE_ID => {
                        MirViewKind::Text
                    }
                    _ => return None,
                };
                Some(MirViewParameterBorrow::new(
                    kind,
                    MirViewLender::Parameter {
                        index: u32::try_from(index).unwrap_or(u32::MAX),
                    },
                    pop_foundation::LifetimeId::from_raw(
                        u32::MAX.saturating_sub(u32::try_from(index).unwrap_or(u32::MAX)),
                    ),
                ))
            })
            .collect();
        let lifetime_summary = pop_types::CallableLifetimeSummary::conservative(
            self.parameters_schema.len(),
            self.results.len(),
        );
        let function = MirFunction {
            function: FunctionId::from_raw(0),
            symbol: self.owner,
            is_async: self.is_async,
            parameter_view_borrows,
            parameters: self.parameters_schema,
            results: self.results,
            lifetime_summary,
            effects: MirEffectSummary::empty(),
            effects_explicit: false,
            blocks,
        };
        (function, self.nested_functions)
    }

    fn initialize_parameter_cells(&mut self) {
        let parameters: Vec<_> = self.cell_parameters.iter().copied().collect();
        for parameter in parameters {
            let initial = self.parameters[&parameter];
            let type_id = self.parameters_schema[parameter.raw() as usize];
            let cell = self.emit(
                MirInstructionKind::CaptureCellAllocate {
                    binding: BindingId::from_raw(parameter.raw()),
                    initial,
                    value_type: type_id,
                    object_map: capture_cell_object_map(self.arena, type_id),
                },
                type_id,
                SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0))),
            );
            self.parameter_cells.insert(parameter, cell);
        }
    }

    fn lower_statements(&mut self, statements: &'hir [HirStatement]) {
        let cleanup_base = self.active_cleanups.len();
        for statement in statements {
            if !matches!(self.current_block().terminator, MirTerminator::Missing) {
                self.current = self.new_block();
            }
            match statement.kind() {
                HirStatementKind::Local {
                    binding,
                    local,
                    local_type,
                    initializer,
                    ..
                } => {
                    let value = self.lower_expression(initializer);
                    if self.cell_locals.contains(local) {
                        let cell = self.emit(
                            MirInstructionKind::CaptureCellAllocate {
                                binding: *binding,
                                initial: value,
                                value_type: *local_type,
                                object_map: capture_cell_object_map(self.arena, *local_type),
                            },
                            *local_type,
                            statement.span(),
                        );
                        self.local_cells.insert(*local, cell);
                    } else {
                        self.locals.insert(*local, value);
                    }
                }
                HirStatementKind::MultipleLocal { bindings, value } => {
                    let value = self.lower_expression(value);
                    for (index, binding) in bindings.iter().enumerate() {
                        let projected = self.emit(
                            MirInstructionKind::TupleGet {
                                tuple: value,
                                index: u32::try_from(index).unwrap_or(u32::MAX),
                            },
                            binding.local_type(),
                            binding.span(),
                        );
                        if self.cell_locals.contains(&binding.local()) {
                            let cell = self.emit(
                                MirInstructionKind::CaptureCellAllocate {
                                    binding: binding.binding(),
                                    initial: projected,
                                    value_type: binding.local_type(),
                                    object_map: capture_cell_object_map(
                                        self.arena,
                                        binding.local_type(),
                                    ),
                                },
                                binding.local_type(),
                                binding.span(),
                            );
                            self.local_cells.insert(binding.local(), cell);
                        } else {
                            self.locals.insert(binding.local(), projected);
                        }
                    }
                }
                HirStatementKind::LocalSet { local, value } => {
                    let value = self.lower_expression(value);
                    if let Some(cell) = self.local_cells.get(local).copied() {
                        self.emit_effect(
                            MirInstructionKind::CaptureCellStore { cell, value },
                            statement.span(),
                        );
                    } else {
                        self.locals.insert(*local, value);
                    }
                }
                HirStatementKind::ParameterSet { parameter, value } => {
                    let value = self.lower_expression(value);
                    if let Some(cell) = self.parameter_cells.get(parameter).copied() {
                        self.emit_effect(
                            MirInstructionKind::CaptureCellStore { cell, value },
                            statement.span(),
                        );
                    } else {
                        self.parameters.insert(*parameter, value);
                    }
                }
                HirStatementKind::CaptureSet { capture, value } => {
                    let value = self.lower_expression(value);
                    let schema = self.capture_schema[capture];
                    self.emit_effect(
                        MirInstructionKind::CaptureStore {
                            capture: *capture,
                            slot: schema.slot(),
                            value,
                        },
                        statement.span(),
                    );
                }
                HirStatementKind::Return { values } => {
                    let values = values
                        .iter()
                        .map(|value| self.lower_expression(value))
                        .collect();
                    self.emit_cleanups_to(0, MirCleanupExitReason::Return);
                    self.terminate(MirTerminator::Return { values });
                }
                HirStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                } => self.lower_if(condition, then_body, else_body),
                HirStatementKind::OptionalIf {
                    local,
                    inner_type,
                    initializer,
                    then_body,
                    else_body,
                    ..
                } => self.lower_optional_if(*local, *inner_type, initializer, then_body, else_body),
                HirStatementKind::While { condition, body } => {
                    self.lower_while(condition, body);
                }
                HirStatementKind::OptionalWhile {
                    local,
                    inner_type,
                    initializer,
                    body,
                    ..
                } => self.lower_optional_while(*local, *inner_type, initializer, body),
                HirStatementKind::RepeatUntil { body, condition } => {
                    self.lower_repeat_until(body, condition);
                }
                HirStatementKind::NumericFor {
                    local,
                    integer_type,
                    first,
                    last,
                    step,
                    body,
                    ..
                } => {
                    self.lower_numeric_for(
                        *local,
                        *integer_type,
                        first,
                        last,
                        step,
                        body,
                        statement.span(),
                    );
                }
                HirStatementKind::GeneralizedFor {
                    protocol,
                    source,
                    item_type,
                    iterator_type,
                    iteration_type,
                    bindings,
                    iterable,
                    body,
                } => self.lower_generalized_for(
                    *protocol,
                    *source,
                    *item_type,
                    *iterator_type,
                    *iteration_type,
                    bindings,
                    iterable,
                    body,
                    statement.span(),
                ),
                HirStatementKind::Break => {
                    let context = self
                        .loop_stack
                        .last()
                        .cloned()
                        .expect("verified HIR resolves break inside a loop");
                    self.emit_cleanups_to(context.cleanup_depth, MirCleanupExitReason::Break);
                    self.branch_with_state_if_open(context.break_target, &context.break_state);
                }
                HirStatementKind::Continue => {
                    let context = self
                        .loop_stack
                        .last()
                        .cloned()
                        .expect("verified HIR resolves continue inside a loop");
                    self.emit_cleanups_to(context.cleanup_depth, MirCleanupExitReason::Continue);
                    self.branch_with_state_if_open(
                        context.continue_target,
                        &context.continue_state,
                    );
                }
                HirStatementKind::Match {
                    scrutinee,
                    union,
                    arms,
                } => {
                    self.lower_match(scrutinee, *union, arms);
                }
                HirStatementKind::ErrorMatch {
                    scrutinee,
                    error,
                    arms,
                } => self.lower_error_match(scrutinee, *error, arms),
                HirStatementKind::ResultMatch {
                    scrutinee,
                    result,
                    result_type,
                    arms,
                } => self.lower_result_match(scrutinee, *result, *result_type, arms),
                HirStatementKind::CodecErrorMatch { scrutinee, arms } => {
                    self.lower_codec_error_match(scrutinee, arms);
                }
                HirStatementKind::Defer { body } => {
                    let scope = CleanupScopeId::from_raw(self.next_cleanup_scope);
                    self.next_cleanup_scope = self.next_cleanup_scope.saturating_add(1);
                    self.active_cleanups.push(ActiveCleanup {
                        scope,
                        action: CleanupAction::Statements(body),
                    });
                }
                HirStatementKind::AsyncDefer { body } => {
                    let scope = CleanupScopeId::from_raw(self.next_cleanup_scope);
                    self.next_cleanup_scope = self.next_cleanup_scope.saturating_add(1);
                    self.active_cleanups.push(ActiveCleanup {
                        scope,
                        action: CleanupAction::Statements(body),
                    });
                }
                HirStatementKind::FieldSet { base, field, value } => {
                    let base = self.lower_expression(base);
                    let value = self.lower_expression(value);
                    if let Some((slot, field_type)) = self.gc_schema.fields.get(field).copied()
                        && is_managed_reference_type_id(field_type, Some(self.arena))
                    {
                        let previous = self.emit(
                            MirInstructionKind::FieldGet {
                                base,
                                field: *field,
                            },
                            field_type,
                            statement.span(),
                        );
                        self.emit_effect(
                            MirInstructionKind::WriteBarrier {
                                owner: base,
                                slot,
                                previous: Some(previous),
                                value: Some(value),
                                proof: None,
                            },
                            statement.span(),
                        );
                    }
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::FieldSet {
                            base,
                            field: *field,
                            value,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::CompoundFieldSet {
                    base,
                    field,
                    value_type,
                    operator,
                    value,
                } => {
                    let base = self.lower_expression(base);
                    let current = self.emit(
                        MirInstructionKind::FieldGet {
                            base,
                            field: *field,
                        },
                        *value_type,
                        statement.span(),
                    );
                    let right = self.lower_expression(value);
                    let value = self.lower_compound_value(
                        *operator,
                        *value_type,
                        current,
                        right,
                        statement.span(),
                    );
                    if let Some((slot, field_type)) = self.gc_schema.fields.get(field).copied()
                        && is_managed_reference_type_id(field_type, Some(self.arena))
                    {
                        let previous = self.emit(
                            MirInstructionKind::FieldGet {
                                base,
                                field: *field,
                            },
                            field_type,
                            statement.span(),
                        );
                        self.emit_effect(
                            MirInstructionKind::WriteBarrier {
                                owner: base,
                                slot,
                                previous: Some(previous),
                                value: Some(value),
                                proof: None,
                            },
                            statement.span(),
                        );
                    }
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::FieldSet {
                            base,
                            field: *field,
                            value,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::ArraySet {
                    array,
                    index,
                    value,
                } => {
                    let element_map = array_element_map(self.arena, array.type_id());
                    let array = self.lower_expression(array);
                    let index = self.lower_expression(index);
                    let value = self.lower_expression(value);
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::ArraySet {
                            array,
                            index,
                            value,
                            element_map,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::ListSet { list, index, value } => {
                    let element_map = list_element_map(self.arena, list.type_id());
                    let list = self.lower_expression(list);
                    let index = self.lower_expression(index);
                    let value = self.lower_expression(value);
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::ListSet {
                            list,
                            index,
                            value,
                            element_map,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::TableSet { table, key, value } => {
                    let table_type = table.type_id();
                    let (key_map, value_map) = table_element_maps(self.arena, table_type);
                    let table = self.lower_expression(table);
                    let key = self.lower_expression(key);
                    let value = self.lower_expression(value);
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::TableSet {
                            table,
                            key,
                            value,
                            key_map,
                            value_map,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::CompoundArraySet {
                    array,
                    index,
                    element_type,
                    operator,
                    value,
                } => {
                    let element_map = array_element_map(self.arena, array.type_id());
                    let array = self.lower_expression(array);
                    let index = self.lower_expression(index);
                    let current = self.emit(
                        MirInstructionKind::ArrayGetChecked { array, index },
                        *element_type,
                        statement.span(),
                    );
                    let right = self.lower_expression(value);
                    let value = self.lower_compound_value(
                        *operator,
                        *element_type,
                        current,
                        right,
                        statement.span(),
                    );
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::ArraySet {
                            array,
                            index,
                            value,
                            element_map,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::MultipleAssignment { targets, value } => {
                    let targets: Vec<_> = targets
                        .iter()
                        .map(|target| self.lower_assignment_target(target, statement.span()))
                        .collect();
                    let value = self.lower_expression(value);
                    for (index, target) in targets.into_iter().enumerate() {
                        let element_type = match &target {
                            LoweredAssignmentTarget::Local { value_type, .. }
                            | LoweredAssignmentTarget::Capture { value_type, .. } => *value_type,
                            LoweredAssignmentTarget::Field { value_type, .. } => *value_type,
                            LoweredAssignmentTarget::Array { element_type, .. } => *element_type,
                            LoweredAssignmentTarget::List { element_type, .. } => *element_type,
                            LoweredAssignmentTarget::Table { value_type, .. } => *value_type,
                        };
                        let projected = self.emit(
                            MirInstructionKind::TupleGet {
                                tuple: value,
                                index: u32::try_from(index).unwrap_or(u32::MAX),
                            },
                            element_type,
                            statement.span(),
                        );
                        self.store_assignment_target(target, projected, statement.span());
                    }
                }
                HirStatementKind::Call(call) => {
                    let kind = self.lower_call(call.dispatch(), call.arguments());
                    self.emit_effect(kind, call.span());
                }
                HirStatementKind::Expression(expression) => {
                    if let HirExpressionKind::FfiHandleClose { handle } = expression.kind() {
                        let handle = self.lower_expression(handle);
                        self.emit_effect(
                            MirInstructionKind::FfiHandleClose { handle },
                            expression.span(),
                        );
                    } else {
                        self.lower_expression(expression);
                    }
                }
            }
        }
        if matches!(self.current_block().terminator, MirTerminator::Missing) {
            self.emit_cleanups_to(cleanup_base, MirCleanupExitReason::Normal);
        }
        self.active_cleanups.truncate(cleanup_base);
    }

    fn emit_cleanups_to(&mut self, depth: usize, reason: MirCleanupExitReason) {
        let registered = self.active_cleanups.clone();
        for index in (depth..registered.len()).rev() {
            let cleanup = registered[index];
            let target = self.new_cleanup_block(MirCleanupBlock {
                scope: cleanup.scope,
                reason,
            });
            self.branch_if_open(target);
            self.current = target;
            self.active_cleanups = registered[..index].to_vec();
            let previous_cleanup = self.current_cleanup.replace(MirCleanupBlock {
                scope: cleanup.scope,
                reason,
            });
            self.lower_cleanup_action(cleanup.action);
            self.current_cleanup = previous_cleanup;
        }
        self.active_cleanups = registered;
        if depth < self.active_cleanups.len() {
            let continuation = self.new_block();
            self.branch_if_open(continuation);
            self.current = continuation;
        }
    }

    fn lower_assignment_target(
        &mut self,
        target: &HirAssignmentTarget,
        span: SourceSpan,
    ) -> LoweredAssignmentTarget {
        match target {
            HirAssignmentTarget::Local {
                local, value_type, ..
            } => LoweredAssignmentTarget::Local {
                local: *local,
                value_type: *value_type,
            },
            HirAssignmentTarget::Capture {
                capture,
                value_type,
                ..
            } => LoweredAssignmentTarget::Capture {
                capture: *capture,
                value_type: *value_type,
            },
            HirAssignmentTarget::Field {
                base,
                field,
                value_type,
            } => LoweredAssignmentTarget::Field {
                base: self.lower_expression(base),
                field: *field,
                value_type: *value_type,
            },
            HirAssignmentTarget::Array {
                array,
                index,
                element_type,
            } => {
                let array_type = array.type_id();
                let array = self.lower_expression(array);
                let index = self.lower_expression(index);
                self.emit(
                    MirInstructionKind::ArrayGetChecked { array, index },
                    *element_type,
                    span,
                );
                LoweredAssignmentTarget::Array {
                    array,
                    index,
                    array_type,
                    element_type: *element_type,
                }
            }
            HirAssignmentTarget::List {
                list,
                index,
                element_type,
            } => {
                let list_type = list.type_id();
                let list = self.lower_expression(list);
                let index = self.lower_expression(index);
                self.emit(
                    MirInstructionKind::ListGetChecked { list, index },
                    *element_type,
                    span,
                );
                LoweredAssignmentTarget::List {
                    list,
                    index,
                    list_type,
                    element_type: *element_type,
                }
            }
            HirAssignmentTarget::Table {
                table,
                key,
                value_type,
            } => {
                let table_type = table.type_id();
                LoweredAssignmentTarget::Table {
                    table: self.lower_expression(table),
                    key: self.lower_expression(key),
                    table_type,
                    value_type: *value_type,
                }
            }
        }
    }

    fn store_assignment_target(
        &mut self,
        target: LoweredAssignmentTarget,
        value: ValueId,
        span: SourceSpan,
    ) {
        match target {
            LoweredAssignmentTarget::Local { local, .. } => {
                if let Some(cell) = self.local_cells.get(&local).copied() {
                    self.emit_effect(MirInstructionKind::CaptureCellStore { cell, value }, span);
                } else {
                    self.locals.insert(local, value);
                }
            }
            LoweredAssignmentTarget::Capture { capture, .. } => {
                let schema = self.capture_schema[&capture];
                self.emit_effect(
                    MirInstructionKind::CaptureStore {
                        capture,
                        slot: schema.slot(),
                        value,
                    },
                    span,
                );
            }
            LoweredAssignmentTarget::Field {
                base,
                field,
                value_type,
            } => {
                if let Some((slot, field_type)) = self.gc_schema.fields.get(&field).copied()
                    && is_managed_reference_type_id(field_type, Some(self.arena))
                {
                    let previous = self.emit(
                        MirInstructionKind::FieldGet { base, field },
                        value_type,
                        span,
                    );
                    self.emit_effect(
                        MirInstructionKind::WriteBarrier {
                            owner: base,
                            slot,
                            previous: Some(previous),
                            value: Some(value),
                            proof: None,
                        },
                        span,
                    );
                }
                let nil = self
                    .arena
                    .source_type("nil")
                    .expect("validated type arena always contains nil");
                self.emit(
                    MirInstructionKind::FieldSet { base, field, value },
                    nil,
                    span,
                );
            }
            LoweredAssignmentTarget::Array {
                array,
                index,
                array_type,
                ..
            } => {
                let nil = self
                    .arena
                    .source_type("nil")
                    .expect("validated type arena always contains nil");
                self.emit(
                    MirInstructionKind::ArraySet {
                        array,
                        index,
                        value,
                        element_map: array_element_map(self.arena, array_type),
                    },
                    nil,
                    span,
                );
            }
            LoweredAssignmentTarget::List {
                list,
                index,
                list_type,
                ..
            } => {
                let nil = self
                    .arena
                    .source_type("nil")
                    .expect("validated type arena always contains nil");
                self.emit(
                    MirInstructionKind::ListSet {
                        list,
                        index,
                        value,
                        element_map: list_element_map(self.arena, list_type),
                    },
                    nil,
                    span,
                );
            }
            LoweredAssignmentTarget::Table {
                table,
                key,
                table_type,
                ..
            } => {
                let (key_map, value_map) = table_element_maps(self.arena, table_type);
                let nil = self
                    .arena
                    .source_type("nil")
                    .expect("validated type arena always contains nil");
                self.emit(
                    MirInstructionKind::TableSet {
                        table,
                        key,
                        value,
                        key_map,
                        value_map,
                    },
                    nil,
                    span,
                );
            }
        }
    }

    fn lower_match(
        &mut self,
        scrutinee: &HirExpression,
        union: SymbolId,
        arms: &'hir [HirMatchArm],
    ) {
        let scrutinee = self.lower_expression(scrutinee);
        let dispatch_block = self.current;
        let join = self.new_block();
        let outer_locals = self.locals.clone();
        let mut switch_arms = Vec::new();
        for arm in arms {
            let specs: Vec<_> = arm
                .bindings()
                .iter()
                .map(|binding| (binding.type_id(), binding.span()))
                .collect();
            let (block, arguments) = self.new_block_with_arguments(&specs);
            switch_arms.push(MirUnionSwitchArm {
                case: arm.case(),
                target: block,
            });
            self.current = block;
            self.locals.clone_from(&outer_locals);
            for (binding, argument) in arm.bindings().iter().zip(arguments) {
                if let Some(local) = binding.local() {
                    self.locals.insert(local, argument);
                }
            }
            self.lower_statements(arm.body());
            self.branch_if_open(join);
        }
        self.locals = outer_locals;
        self.current = dispatch_block;
        self.terminate(MirTerminator::UnionSwitch {
            scrutinee,
            union,
            arms: switch_arms,
        });
        self.current = join;
    }

    fn lower_error_match(
        &mut self,
        scrutinee: &HirExpression,
        error: pop_foundation::ErrorId,
        arms: &'hir [HirErrorMatchArm],
    ) {
        let scrutinee = self.lower_expression(scrutinee);
        let dispatch_block = self.current;
        let join = self.new_block();
        let outer_locals = self.locals.clone();
        let mut switch_arms = Vec::new();
        for arm in arms {
            let specs: Vec<_> = arm
                .bindings()
                .iter()
                .map(|binding| (binding.type_id(), binding.span()))
                .collect();
            let (block, arguments) = self.new_block_with_arguments(&specs);
            switch_arms.push(MirErrorSwitchArm {
                case: arm.case(),
                target: block,
            });
            self.current = block;
            self.locals.clone_from(&outer_locals);
            for (binding, argument) in arm.bindings().iter().zip(arguments) {
                if let Some(local) = binding.local() {
                    self.locals.insert(local, argument);
                }
            }
            self.lower_statements(arm.body());
            self.branch_if_open(join);
        }
        self.locals = outer_locals;
        self.current = dispatch_block;
        self.terminate(MirTerminator::ErrorSwitch {
            scrutinee,
            error,
            arms: switch_arms,
        });
        self.current = join;
    }

    fn lower_codec_error_match(
        &mut self,
        scrutinee: &HirExpression,
        arms: &'hir [HirCodecErrorMatchArm],
    ) {
        let scrutinee = self.lower_expression(scrutinee);
        let dispatch_block = self.current;
        let join = self.new_block();
        let outer_locals = self.locals.clone();
        let mut switch_arms = Vec::new();
        for arm in arms {
            let block = self.new_block();
            switch_arms.push(MirCodecErrorSwitchArm {
                case: arm.case(),
                target: block,
            });
            self.current = block;
            self.locals.clone_from(&outer_locals);
            self.lower_statements(arm.body());
            self.branch_if_open(join);
        }
        self.locals = outer_locals;
        self.current = dispatch_block;
        self.terminate(MirTerminator::CodecErrorSwitch {
            scrutinee,
            arms: switch_arms,
        });
        self.current = join;
    }

    fn lower_result_match(
        &mut self,
        scrutinee: &HirExpression,
        result_definition: pop_foundation::BuiltinTypeId,
        _result_type: TypeId,
        arms: &'hir [HirResultMatchArm],
    ) {
        let result_value = self.lower_expression(scrutinee);
        let is_ok = self.emit(
            MirInstructionKind::ResultIsOk {
                result: result_value,
                definition: result_definition,
            },
            self.arena.source_type("Boolean").expect("Boolean"),
            scrutinee.span(),
        );
        let dispatch = self.current;
        let join = self.new_block();
        let outer_locals = self.locals.clone();
        let mut ok_block = None;
        let mut error_block = None;
        for arm in arms {
            let block = self.new_block();
            if arm.case() == ResultCaseId::from_raw(0) {
                ok_block = Some(block);
            } else {
                error_block = Some(block);
            }
            self.current = block;
            self.locals.clone_from(&outer_locals);
            let binding = &arm.bindings()[0];
            let value = self.emit(
                if arm.case() == ResultCaseId::from_raw(0) {
                    MirInstructionKind::ResultGetOk {
                        result: result_value,
                        definition: result_definition,
                    }
                } else {
                    MirInstructionKind::ResultGetError {
                        result: result_value,
                        definition: result_definition,
                    }
                },
                binding.type_id(),
                binding.span(),
            );
            if let Some(local) = binding.local() {
                self.locals.insert(local, value);
            }
            self.lower_statements(arm.body());
            self.branch_if_open(join);
        }
        self.locals = outer_locals;
        let ok_block = ok_block.expect("verified Result match has Ok arm");
        let error_block = error_block.expect("verified Result match has Error arm");
        self.current = dispatch;
        self.terminate(MirTerminator::ConditionalBranch {
            condition: is_ok,
            when_true: ok_block,
            when_false: error_block,
        });
        self.current = join;
    }

    fn lower_if(
        &mut self,
        condition: &HirExpression,
        then_body: &'hir [HirStatement],
        else_body: &'hir [HirStatement],
    ) {
        let condition_span = condition.span();
        let condition = self.lower_expression(condition);
        let state = self.live_state(condition_span);
        let then_block = self.new_block();
        let else_block = self.new_block();
        let (join_block, join_arguments) = self.new_block_with_arguments(&state.specs);
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: then_block,
            when_false: else_block,
        });
        let outer_parameters = self.parameters.clone();
        let outer_locals = self.locals.clone();
        self.current = then_block;
        self.lower_statements(then_body);
        let then_reaches_join = matches!(self.current_block().terminator, MirTerminator::Missing);
        self.branch_with_state_if_open(join_block, &state);
        self.parameters.clone_from(&outer_parameters);
        self.locals.clone_from(&outer_locals);
        self.current = else_block;
        self.lower_statements(else_body);
        let else_reaches_join = matches!(self.current_block().terminator, MirTerminator::Missing);
        self.branch_with_state_if_open(join_block, &state);
        self.current = join_block;
        if !then_reaches_join && !else_reaches_join {
            self.terminate(MirTerminator::Unreachable);
            return;
        }
        self.install_state(&state, &join_arguments);
    }

    fn lower_optional_if(
        &mut self,
        local: LocalId,
        inner_type: TypeId,
        initializer: &HirExpression,
        then_body: &'hir [HirStatement],
        else_body: &'hir [HirStatement],
    ) {
        let optional = self.lower_expression(initializer);
        let present = self.emit(
            MirInstructionKind::OptionalIsPresent { optional },
            self.arena.source_type("Boolean").expect("Boolean"),
            initializer.span(),
        );
        let state = self.live_state(initializer.span());
        let then_block = self.new_block();
        let else_block = self.new_block();
        let (join_block, join_arguments) = self.new_block_with_arguments(&state.specs);
        self.terminate(MirTerminator::ConditionalBranch {
            condition: present,
            when_true: then_block,
            when_false: else_block,
        });

        let outer_parameters = self.parameters.clone();
        let outer_locals = self.locals.clone();
        self.current = then_block;
        let value = self.emit(
            MirInstructionKind::OptionalGet { optional },
            inner_type,
            initializer.span(),
        );
        self.locals.insert(local, value);
        self.lower_statements(then_body);
        let then_reaches_join = matches!(self.current_block().terminator, MirTerminator::Missing);
        self.branch_with_state_if_open(join_block, &state);

        self.parameters = outer_parameters;
        self.locals = outer_locals;
        self.current = else_block;
        self.lower_statements(else_body);
        let else_reaches_join = matches!(self.current_block().terminator, MirTerminator::Missing);
        self.branch_with_state_if_open(join_block, &state);

        self.current = join_block;
        if !then_reaches_join && !else_reaches_join {
            self.terminate(MirTerminator::Unreachable);
            return;
        }
        self.install_state(&state, &join_arguments);
    }

    fn lower_while(&mut self, condition: &HirExpression, body: &'hir [HirStatement]) {
        let state = self.live_state(condition.span());
        let initial_values = self.state_values(&state);
        let (condition_block, condition_arguments) = self.new_block_with_arguments(&state.specs);
        let body_block = self.new_block();
        let exit_edge = self.new_block();
        let (exit_block, exit_arguments) = self.new_block_with_arguments(&state.specs);
        self.branch_with_arguments_if_open(condition_block, initial_values);
        self.current = condition_block;
        self.install_state(&state, &condition_arguments);
        let condition = self.lower_expression(condition);
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: body_block,
            when_false: exit_edge,
        });
        self.current = body_block;
        self.loop_stack.push(LoopContext {
            break_target: exit_block,
            break_state: state.clone(),
            continue_target: condition_block,
            continue_state: state.clone(),
            cleanup_depth: self.active_cleanups.len(),
        });
        self.lower_statements(body);
        self.loop_stack
            .pop()
            .expect("while loop context was pushed");
        self.branch_with_state_if_open(condition_block, &state);
        self.current = exit_edge;
        self.install_state(&state, &condition_arguments);
        self.branch_with_state_if_open(exit_block, &state);
        self.current = exit_block;
        self.install_state(&state, &exit_arguments);
    }

    fn lower_optional_while(
        &mut self,
        local: LocalId,
        inner_type: TypeId,
        initializer: &HirExpression,
        body: &'hir [HirStatement],
    ) {
        let state = self.live_state(initializer.span());
        let initial_values = self.state_values(&state);
        let (condition_block, condition_arguments) = self.new_block_with_arguments(&state.specs);
        let body_block = self.new_block();
        let exit_edge = self.new_block();
        let (exit_block, exit_arguments) = self.new_block_with_arguments(&state.specs);
        self.branch_with_arguments_if_open(condition_block, initial_values);

        self.current = condition_block;
        self.install_state(&state, &condition_arguments);
        let optional = self.lower_expression(initializer);
        let present = self.emit(
            MirInstructionKind::OptionalIsPresent { optional },
            self.arena.source_type("Boolean").expect("Boolean"),
            initializer.span(),
        );
        self.terminate(MirTerminator::ConditionalBranch {
            condition: present,
            when_true: body_block,
            when_false: exit_edge,
        });

        self.current = body_block;
        let value = self.emit(
            MirInstructionKind::OptionalGet { optional },
            inner_type,
            initializer.span(),
        );
        self.locals.insert(local, value);
        self.loop_stack.push(LoopContext {
            break_target: exit_block,
            break_state: state.clone(),
            continue_target: condition_block,
            continue_state: state.clone(),
            cleanup_depth: self.active_cleanups.len(),
        });
        self.lower_statements(body);
        self.loop_stack
            .pop()
            .expect("optional while loop context was pushed");
        self.branch_with_state_if_open(condition_block, &state);

        self.current = exit_edge;
        self.install_state(&state, &condition_arguments);
        self.branch_with_state_if_open(exit_block, &state);
        self.current = exit_block;
        self.locals.remove(&local);
        self.install_state(&state, &exit_arguments);
    }

    fn lower_repeat_until(&mut self, body: &'hir [HirStatement], condition: &HirExpression) {
        let state = self.live_state(condition.span());
        let initial_values = self.state_values(&state);
        let outer_locals = self.locals.clone();
        let has_continue = contains_continue_for_current_loop(body);
        let (body_block, body_arguments) = self.new_block_with_arguments(&state.specs);
        let (condition_block, condition_arguments) = if has_continue {
            self.new_block_with_arguments(&state.specs)
        } else {
            (self.new_block(), Vec::new())
        };
        let repeat_edge = self.new_block();
        let exit_edge = self.new_block();
        let (exit_block, exit_arguments) = self.new_block_with_arguments(&state.specs);

        self.branch_with_arguments_if_open(body_block, initial_values);
        self.current = body_block;
        self.install_state(&state, &body_arguments);
        self.loop_stack.push(LoopContext {
            break_target: exit_block,
            break_state: state.clone(),
            continue_target: condition_block,
            continue_state: state.clone(),
            cleanup_depth: self.active_cleanups.len(),
        });
        self.lower_statements(body);
        self.loop_stack
            .pop()
            .expect("repeat loop context was pushed");
        if has_continue {
            self.branch_with_state_if_open(condition_block, &state);
        } else {
            self.branch_if_open(condition_block);
        }

        self.current = condition_block;
        if has_continue {
            self.install_state(&state, &condition_arguments);
        }
        let condition = self.lower_expression(condition);
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: exit_edge,
            when_false: repeat_edge,
        });

        self.current = repeat_edge;
        self.branch_with_state_if_open(body_block, &state);

        self.current = exit_edge;
        self.branch_with_state_if_open(exit_block, &state);

        self.current = exit_block;
        self.locals = outer_locals;
        self.install_state(&state, &exit_arguments);
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn lower_numeric_for(
        &mut self,
        local: LocalId,
        integer_type: TypeId,
        first: &HirExpression,
        last: &HirExpression,
        step: &HirExpression,
        body: &'hir [HirStatement],
        span: SourceSpan,
    ) {
        let first = self.lower_expression(first);
        let last = self.lower_expression(last);
        let step = self.lower_expression(step);
        let kind = integer_kind(self.arena, integer_type)
            .expect("verified numeric for range has one fixed integer type");
        let zero = self.emit(
            MirInstructionKind::IntegerConstant(
                IntegerValue::parse_decimal("0", kind).expect("zero fits every integer"),
            ),
            integer_type,
            span,
        );
        let step_is_zero = self.emit(
            MirInstructionKind::CompareEqual {
                left: step,
                right: zero,
            },
            self.arena
                .source_type("Boolean")
                .expect("validated type arena contains Boolean"),
            span,
        );

        let outer_state = self.live_state(span);
        let initial_outer_values = self.state_values(&outer_state);
        let outer_locals = self.locals.clone();
        let mut loop_specs = outer_state.specs.clone();
        loop_specs.push((integer_type, span));
        let (condition_block, condition_arguments) = self.new_block_with_arguments(&loop_specs);
        let initial_edge = self.new_block();
        let positive_condition = self.new_block();
        let negative_condition = self.new_block();
        let body_block = self.new_block();
        let condition_exit_edge = self.new_block();
        let mut continue_specs = outer_state.specs.clone();
        continue_specs.push((integer_type, span));
        let (increment_block, increment_arguments) = self.new_block_with_arguments(&continue_specs);
        let increment_exit_edge = self.new_block();
        let advance_block = self.new_block();
        let invalid_step = self.new_block();
        let (exit_block, exit_arguments) = self.new_block_with_arguments(&outer_state.specs);

        self.terminate(MirTerminator::ConditionalBranch {
            condition: step_is_zero,
            when_true: invalid_step,
            when_false: initial_edge,
        });
        self.current = invalid_step;
        self.terminate(MirTerminator::Trap(Trap::new(TrapKind::InvalidRangeStep)));

        self.current = initial_edge;
        let mut initial_values = initial_outer_values;
        initial_values.push(first);
        self.branch_with_arguments_if_open(condition_block, initial_values);

        self.current = condition_block;
        self.install_state(
            &outer_state,
            &condition_arguments[..outer_state.specs.len()],
        );
        let induction = condition_arguments[outer_state.specs.len()];
        let condition = if kind.is_signed() {
            self.emit(
                MirInstructionKind::CompareIntegerGreater {
                    kind,
                    left: step,
                    right: zero,
                },
                self.arena.source_type("Boolean").expect("Boolean"),
                span,
            )
        } else {
            self.emit(
                MirInstructionKind::BooleanConstant(true),
                self.arena.source_type("Boolean").expect("Boolean"),
                span,
            )
        };
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: positive_condition,
            when_false: negative_condition,
        });

        self.current = positive_condition;
        let in_positive_range = self.emit(
            MirInstructionKind::CompareIntegerLessOrEqual {
                kind,
                left: induction,
                right: last,
            },
            self.arena.source_type("Boolean").expect("Boolean"),
            span,
        );
        self.terminate(MirTerminator::ConditionalBranch {
            condition: in_positive_range,
            when_true: body_block,
            when_false: condition_exit_edge,
        });

        self.current = negative_condition;
        let in_negative_range = self.emit(
            MirInstructionKind::CompareIntegerGreaterOrEqual {
                kind,
                left: induction,
                right: last,
            },
            self.arena.source_type("Boolean").expect("Boolean"),
            span,
        );
        self.terminate(MirTerminator::ConditionalBranch {
            condition: in_negative_range,
            when_true: body_block,
            when_false: condition_exit_edge,
        });

        self.current = body_block;
        self.locals.insert(local, induction);
        let mut continue_state = outer_state.clone();
        continue_state.locals.push(local);
        continue_state.specs.push((integer_type, span));
        self.loop_stack.push(LoopContext {
            break_target: exit_block,
            break_state: outer_state.clone(),
            continue_target: increment_block,
            continue_state: continue_state.clone(),
            cleanup_depth: self.active_cleanups.len(),
        });
        self.lower_statements(body);
        self.loop_stack
            .pop()
            .expect("numeric for context was pushed");
        self.branch_with_state_if_open(increment_block, &continue_state);

        self.current = increment_block;
        self.install_state(
            &outer_state,
            &increment_arguments[..outer_state.specs.len()],
        );
        let current = increment_arguments[outer_state.specs.len()];
        self.locals.insert(local, current);
        let reached_last = self.emit(
            MirInstructionKind::CompareEqual {
                left: current,
                right: last,
            },
            self.arena.source_type("Boolean").expect("Boolean"),
            span,
        );
        self.terminate(MirTerminator::ConditionalBranch {
            condition: reached_last,
            when_true: increment_exit_edge,
            when_false: advance_block,
        });

        self.current = advance_block;
        let next = self.emit(
            MirInstructionKind::CheckedIntegerAdd {
                kind,
                left: current,
                right: step,
            },
            integer_type,
            span,
        );
        let mut next_values = self.state_values(&outer_state);
        next_values.push(next);
        self.branch_with_arguments_if_open(condition_block, next_values);

        self.current = condition_exit_edge;
        self.install_state(
            &outer_state,
            &condition_arguments[..outer_state.specs.len()],
        );
        self.branch_with_state_if_open(exit_block, &outer_state);
        self.current = increment_exit_edge;
        self.install_state(
            &outer_state,
            &increment_arguments[..outer_state.specs.len()],
        );
        self.branch_with_state_if_open(exit_block, &outer_state);

        self.current = exit_block;
        self.locals = outer_locals;
        self.install_state(&outer_state, &exit_arguments);
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_generalized_for(
        &mut self,
        protocol: HirIterationProtocol,
        source: HirIterationSource,
        item_type: TypeId,
        iterator_type: TypeId,
        iteration_type: TypeId,
        bindings: &[HirLocalBinding],
        iterable: &HirExpression,
        body: &'hir [HirStatement],
        span: SourceSpan,
    ) {
        let source_value = self.lower_expression(iterable);
        let iterator = match source {
            HirIterationSource::ClassIterable { iterator_method }
            | HirIterationSource::ClassIterator {
                iterator_method, ..
            } => self.emit(
                MirInstructionKind::CallDirectMethod {
                    method: iterator_method,
                    arguments: vec![source_value],
                    declared_effects: self
                        .method_effects
                        .get(&iterator_method)
                        .copied()
                        .unwrap_or_default(),
                    unwind: MirUnwindAction::Propagate,
                },
                iterator_type,
                span,
            ),
            _ => {
                let acquisition_interface = if source == HirIterationSource::Iterator {
                    protocol.iterator()
                } else {
                    protocol.iterable()
                };
                self.emit(
                    MirInstructionKind::CallBuiltinInterface {
                        interface: acquisition_interface,
                        method: protocol.iterator_method(),
                        arguments: vec![source_value],
                        declared_effects: self
                            .builtin_interface_effects
                            .get(&(acquisition_interface, protocol.iterator_method()))
                            .copied()
                            .unwrap_or_default(),
                        unwind: MirUnwindAction::Propagate,
                    },
                    iterator_type,
                    span,
                )
            }
        };

        let outer_state = self.live_state(span);
        let initial_values = self.state_values(&outer_state);
        let outer_locals = self.locals.clone();
        let (step_block, step_arguments) = self.new_block_with_arguments(&outer_state.specs);
        let body_block = self.new_block();
        let exit_edge = self.new_block();
        let (exit_block, exit_arguments) = self.new_block_with_arguments(&outer_state.specs);
        self.branch_with_arguments_if_open(step_block, initial_values);

        self.current = step_block;
        self.install_state(&outer_state, &step_arguments);
        let iteration = if let HirIterationSource::ClassIterator { next_method, .. } = source {
            self.emit(
                MirInstructionKind::CallDirectMethod {
                    method: next_method,
                    arguments: vec![source_value],
                    declared_effects: self
                        .method_effects
                        .get(&next_method)
                        .copied()
                        .unwrap_or_default(),
                    unwind: MirUnwindAction::Propagate,
                },
                iteration_type,
                span,
            )
        } else {
            self.emit(
                MirInstructionKind::CallBuiltinInterface {
                    interface: protocol.iterator(),
                    method: protocol.next_method(),
                    arguments: vec![iterator],
                    declared_effects: self
                        .builtin_interface_effects
                        .get(&(protocol.iterator(), protocol.next_method()))
                        .copied()
                        .unwrap_or_default(),
                    unwind: MirUnwindAction::Propagate,
                },
                iteration_type,
                span,
            )
        };
        let has_item = self.emit(
            MirInstructionKind::IterationIsItem {
                iteration,
                definition: protocol.iteration(),
                item_case: protocol.item_case(),
                end_case: protocol.end_case(),
            },
            self.arena.source_type("Boolean").expect("Boolean"),
            span,
        );
        self.terminate(MirTerminator::ConditionalBranch {
            condition: has_item,
            when_true: body_block,
            when_false: exit_edge,
        });

        self.current = body_block;
        let item = self.emit(
            MirInstructionKind::IterationGetItem {
                iteration,
                definition: protocol.iteration(),
                item_case: protocol.item_case(),
            },
            item_type,
            span,
        );
        for (index, binding) in bindings.iter().enumerate() {
            let value = if bindings.len() == 1 {
                item
            } else {
                self.emit(
                    MirInstructionKind::TupleGet {
                        tuple: item,
                        index: u32::try_from(index).unwrap_or(u32::MAX),
                    },
                    binding.local_type(),
                    binding.span(),
                )
            };
            if self.cell_locals.contains(&binding.local()) {
                let cell = self.emit(
                    MirInstructionKind::CaptureCellAllocate {
                        binding: binding.binding(),
                        initial: value,
                        value_type: binding.local_type(),
                        object_map: capture_cell_object_map(self.arena, binding.local_type()),
                    },
                    binding.local_type(),
                    binding.span(),
                );
                self.local_cells.insert(binding.local(), cell);
            } else {
                self.locals.insert(binding.local(), value);
            }
        }
        self.loop_stack.push(LoopContext {
            break_target: exit_block,
            break_state: outer_state.clone(),
            continue_target: step_block,
            continue_state: outer_state.clone(),
            cleanup_depth: self.active_cleanups.len(),
        });
        self.lower_statements(body);
        self.loop_stack
            .pop()
            .expect("generalized for context was pushed");
        self.branch_with_state_if_open(step_block, &outer_state);

        self.current = exit_edge;
        self.install_state(&outer_state, &step_arguments);
        self.branch_with_state_if_open(exit_block, &outer_state);
        self.current = exit_block;
        self.locals = outer_locals;
        for binding in bindings {
            self.local_cells.remove(&binding.local());
        }
        self.install_state(&outer_state, &exit_arguments);
    }

    #[allow(clippy::too_many_lines)]
    fn lower_expression(&mut self, expression: &HirExpression) -> ValueId {
        let kind = match expression.kind() {
            HirExpressionKind::Integer(value) => MirInstructionKind::IntegerConstant(*value),
            HirExpressionKind::Float(value) => MirInstructionKind::FloatConstant(*value),
            HirExpressionKind::String(value) => MirInstructionKind::StringConstant(value.clone()),
            HirExpressionKind::Boolean(value) => MirInstructionKind::BooleanConstant(*value),
            HirExpressionKind::Nil => MirInstructionKind::NilConstant,
            HirExpressionKind::EnumCase {
                definition,
                case,
                discriminant,
            } => MirInstructionKind::EnumConstant {
                definition: *definition,
                case: *case,
                discriminant: *discriminant,
            },
            HirExpressionKind::CodecErrorCase(case) => {
                MirInstructionKind::CodecErrorConstant { case: *case }
            }
            HirExpressionKind::GeneratedCodecSchema(adapter) => {
                MirInstructionKind::GeneratedCodecSchema(*adapter)
            }
            HirExpressionKind::Closure(closure) => {
                return self.lower_closure(closure, expression.type_id());
            }
            HirExpressionKind::FfiWithCallback {
                callback,
                callback_type,
                binding_contract,
                body,
                site,
                region,
                ..
            } => {
                let callback_value = self.lower_closure(callback, *callback_type);
                let registered_type = self.registered_callback_type(*callback_type);
                let registered = self.emit(
                    MirInstructionKind::FfiCallbackOpenScoped {
                        callback: callback_value,
                        callback_type: *callback_type,
                        owner: self.owner,
                        function: callback.function(),
                        site: *site,
                        region: *region,
                    },
                    registered_type,
                    expression.span(),
                );
                let cleanup_scope = CleanupScopeId::from_raw(self.next_cleanup_scope);
                self.next_cleanup_scope = self.next_cleanup_scope.saturating_add(1);
                self.active_cleanups.push(ActiveCleanup {
                    scope: cleanup_scope,
                    action: CleanupAction::FfiCallback {
                        callback: registered,
                        region: *region,
                        span: expression.span(),
                    },
                });
                let (captures, declared_effects) = self.lower_scoped_closure(body);
                let result = self.emit(
                    MirInstructionKind::CallCallbackPair {
                        callback: registered,
                        signature: self.ffi_callback_signature(*callback_type, binding_contract),
                        owner: self.owner,
                        function: body.function(),
                        captures,
                        region: *region,
                        lifetime: FfiCallbackLifetime::CallScoped,
                        result: None,
                        success: None,
                        failure: None,
                        declared_effects,
                        unwind: MirUnwindAction::Propagate,
                    },
                    expression.type_id(),
                    expression.span(),
                );
                self.active_cleanups
                    .pop()
                    .expect("scoped FFI callback cleanup was registered");
                self.emit_effect(
                    MirInstructionKind::FfiCallbackCloseScoped {
                        callback: registered,
                        region: *region,
                    },
                    expression.span(),
                );
                return result;
            }
            HirExpressionKind::FfiCallbackOpen {
                callback,
                callback_type,
                thread,
                site,
            } => {
                let callback_value = self.lower_closure(callback, *callback_type);
                let result = self.result_definition(expression.type_id());
                return self.emit(
                    MirInstructionKind::FfiCallbackOpenOwned {
                        callback: callback_value,
                        callback_type: *callback_type,
                        owner: self.owner,
                        function: callback.function(),
                        site: *site,
                        thread: match thread {
                            pop_types::FfiCallbackThreadPolicy::CallingThread => {
                                FfiCallbackThread::CallingThread
                            }
                            pop_types::FfiCallbackThreadPolicy::AttachedThread => {
                                FfiCallbackThread::AttachedThread
                            }
                        },
                        result,
                        success: ResultCaseId::from_raw(0),
                        failure: ResultCaseId::from_raw(1),
                    },
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::FfiCallbackWithPair {
                callback,
                callback_type,
                binding_contract,
                body,
                region,
                ..
            } => {
                let callback = self.lower_expression(callback);
                let (captures, declared_effects) = self.lower_scoped_closure(body);
                let result = self.result_definition(expression.type_id());
                return self.emit(
                    MirInstructionKind::CallCallbackPair {
                        callback,
                        signature: self.ffi_callback_signature(*callback_type, binding_contract),
                        owner: self.owner,
                        function: body.function(),
                        captures,
                        region: *region,
                        lifetime: FfiCallbackLifetime::Registered,
                        result: Some(result),
                        success: Some(ResultCaseId::from_raw(0)),
                        failure: Some(ResultCaseId::from_raw(1)),
                        declared_effects,
                        unwind: MirUnwindAction::Propagate,
                    },
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::FfiCallbackClose { callback, .. } => {
                let callback = self.lower_expression(callback);
                let result = self.result_definition(expression.type_id());
                MirInstructionKind::FfiCallbackCloseOwned {
                    callback,
                    result,
                    success: ResultCaseId::from_raw(0),
                    failure: ResultCaseId::from_raw(1),
                }
            }
            HirExpressionKind::Local(local) => {
                if let Some(cell) = self.local_cells.get(local).copied() {
                    return self.emit(
                        MirInstructionKind::CaptureCellLoad { cell },
                        expression.type_id(),
                        expression.span(),
                    );
                }
                return self.locals[local];
            }
            HirExpressionKind::Parameter(parameter) => {
                if let Some(cell) = self.parameter_cells.get(parameter).copied() {
                    return self.emit(
                        MirInstructionKind::CaptureCellLoad { cell },
                        expression.type_id(),
                        expression.span(),
                    );
                }
                return self.parameters[parameter];
            }
            HirExpressionKind::Capture(capture) => {
                let schema = self.capture_schema[capture];
                return self.emit(
                    MirInstructionKind::CaptureLoad {
                        capture: *capture,
                        slot: schema.slot(),
                        mode: schema.mode(),
                    },
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::Function(function) => {
                MirInstructionKind::FunctionReference(*function)
            }
            HirExpressionKind::Tuple(elements) => MirInstructionKind::TupleMake(
                elements
                    .iter()
                    .map(|element| self.lower_expression(element))
                    .collect(),
            ),
            HirExpressionKind::StringConcat { left, right } => MirInstructionKind::StringConcat {
                left: self.lower_expression(left),
                right: self.lower_expression(right),
            },
            HirExpressionKind::StringFormat { kind, value } => MirInstructionKind::StringFormat {
                kind: *kind,
                value: self.lower_expression(value),
            },
            HirExpressionKind::Array(elements) => MirInstructionKind::ArrayMake {
                elements: elements
                    .iter()
                    .map(|element| self.lower_expression(element))
                    .collect(),
                element_map: array_element_map(self.arena, expression.type_id()),
            },
            HirExpressionKind::ArrayCreate {
                length,
                initial_value,
            } => MirInstructionKind::ArrayCreate {
                length: self.lower_expression(length),
                initial_value: self.lower_expression(initial_value),
                element_map: array_element_map(self.arena, expression.type_id()),
            },
            HirExpressionKind::Table(entries) => {
                let (key_map, value_map) = table_element_maps(self.arena, expression.type_id());
                MirInstructionKind::TableMake {
                    key_map,
                    value_map,
                    entries: self.lower_table_entries(entries),
                }
            }
            HirExpressionKind::TableGet { table, key } => MirInstructionKind::TableGet {
                table: self.lower_expression(table),
                key: self.lower_expression(key),
            },
            HirExpressionKind::Unary { operator, operand } => {
                let operand = self.lower_expression(operand);
                match operator {
                    TypedUnaryOperator::Not => MirInstructionKind::BooleanNot { operand },
                    TypedUnaryOperator::Negate => {
                        if let Some(kind) = integer_kind(self.arena, expression.type_id()) {
                            MirInstructionKind::IntegerNegate { kind, operand }
                        } else {
                            MirInstructionKind::FloatNegate {
                                kind: float_kind(self.arena, expression.type_id())
                                    .expect("typed numeric negation has a numeric type"),
                                operand,
                            }
                        }
                    }
                }
            }
            HirExpressionKind::Binary {
                operator,
                left,
                right,
            } => return self.lower_binary_expression(expression, *operator, left, right),
            HirExpressionKind::OptionalDefault { optional, fallback } => {
                return self.lower_optional_default(
                    optional,
                    fallback,
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::OptionalPropagate {
                optional,
                enclosing_result,
            } => {
                return self.lower_optional_propagate(
                    optional,
                    *enclosing_result,
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::ResultPropagate {
                result,
                result_definition,
                success_type,
                error_type,
                enclosing_result,
            } => {
                return self.lower_result_propagate(
                    result,
                    *result_definition,
                    *success_type,
                    *error_type,
                    *enclosing_result,
                    expression.span(),
                );
            }
            HirExpressionKind::OptionalNarrow { optional } => {
                let optional = self.lower_expression(optional);
                return self.emit(
                    MirInstructionKind::OptionalGet { optional },
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::Conditional {
                condition,
                when_true,
                when_false,
            } => {
                return self.lower_conditional_expression(
                    condition,
                    when_true,
                    when_false,
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::Call {
                dispatch,
                is_async,
                arguments,
                ..
            } => {
                if *is_async {
                    return self.lower_task_create(
                        dispatch,
                        arguments,
                        expression.type_id(),
                        expression.span(),
                    );
                }
                self.lower_call(dispatch, arguments)
            }
            HirExpressionKind::Await { task } => {
                return self.lower_await(task, expression.type_id(), expression.span());
            }
            HirExpressionKind::TaskCancellationSource => MirInstructionKind::CancelSourceCreate,
            HirExpressionKind::TaskCancelToken { source } => {
                MirInstructionKind::CancelSourceToken {
                    source: self.lower_expression(source),
                }
            }
            HirExpressionKind::TaskCancel { source } => MirInstructionKind::CancelRequest {
                source: self.lower_expression(source),
            },
            HirExpressionKind::TaskGroup { cancel, body } => {
                let completion_type = match self.arena.get(expression.type_id()) {
                    Some(SemanticType::Builtin { arguments, .. }) if arguments.len() == 1 => {
                        arguments[0]
                    }
                    _ => unreachable!("verified task groups produce exact Task<T> values"),
                };
                let cancel = self.lower_expression(cancel);
                let body = self.lower_expression(body);
                let object_map = task_group_object_map(
                    self.value_type(cancel),
                    self.value_type(body),
                    completion_type,
                    self.arena,
                );
                MirInstructionKind::TaskGroupCreate {
                    cancel,
                    body,
                    completion_type,
                    object_map,
                }
            }
            HirExpressionKind::TaskStart { group, task } => MirInstructionKind::TaskStart {
                group: self.lower_expression(group),
                task: self.lower_expression(task),
            },
            HirExpressionKind::FfiHandleOpen { value } => MirInstructionKind::FfiHandleOpen {
                value: self.lower_expression(value),
            },
            HirExpressionKind::FfiHandleGet { handle } => MirInstructionKind::FfiHandleGet {
                handle: self.lower_expression(handle),
            },
            HirExpressionKind::FfiHandleClose { handle } => {
                let handle = self.lower_expression(handle);
                self.emit_effect(
                    MirInstructionKind::FfiHandleClose { handle },
                    expression.span(),
                );
                MirInstructionKind::NilConstant
            }
            HirExpressionKind::FfiBufferOpen {
                length, element, ..
            } => {
                let (layout, element_size, alignment) = self.ffi_layout(*element);
                let result = match self.arena.get(expression.type_id()) {
                    Some(SemanticType::Builtin { definition, .. }) => *definition,
                    _ => unreachable!("verified FFI buffer allocation has an exact Result type"),
                };
                MirInstructionKind::FfiBufferOpen {
                    length: self.lower_expression(length),
                    element: *element,
                    layout,
                    element_size,
                    alignment,
                    result,
                    success: ResultCaseId::from_raw(0),
                    failure: ResultCaseId::from_raw(1),
                }
            }
            HirExpressionKind::FfiBufferLength { buffer } => {
                let element = ffi_buffer_type_element(self.arena, buffer.type_id())
                    .expect("verified FFI buffer operand has one element type");
                let (layout, _, _) = self.ffi_layout(element);
                MirInstructionKind::FfiBufferLength {
                    buffer: self.lower_expression(buffer),
                    layout,
                }
            }
            HirExpressionKind::FfiBufferRead { buffer, index } => {
                let (layout, _, _) = self.ffi_layout(expression.type_id());
                MirInstructionKind::FfiBufferRead {
                    buffer: self.lower_expression(buffer),
                    index: self.lower_expression(index),
                    layout,
                }
            }
            HirExpressionKind::FfiBufferWrite {
                buffer,
                index,
                value,
            } => {
                let (layout, _, _) = self.ffi_layout(value.type_id());
                let buffer = self.lower_expression(buffer);
                let index = self.lower_expression(index);
                let value = self.lower_expression(value);
                self.emit_effect(
                    MirInstructionKind::FfiBufferWrite {
                        buffer,
                        index,
                        value,
                        layout,
                    },
                    expression.span(),
                );
                MirInstructionKind::NilConstant
            }
            HirExpressionKind::FfiBufferClose { buffer } => {
                let buffer = self.lower_expression(buffer);
                self.emit_effect(
                    MirInstructionKind::FfiBufferClose { buffer },
                    expression.span(),
                );
                MirInstructionKind::NilConstant
            }
            HirExpressionKind::FfiBufferWithPointer {
                buffer,
                body,
                element,
                region,
                ..
            } => {
                let buffer = self.lower_expression(buffer);
                let (layout, _, _) = self.ffi_layout(*element);
                let length = self.emit(
                    MirInstructionKind::FfiBufferLength { buffer, layout },
                    body.parameters()[1].type_id(),
                    expression.span(),
                );
                let pointer = self.emit(
                    MirInstructionKind::FfiBufferBorrow {
                        buffer,
                        expected_length: length,
                        layout,
                        region: *region,
                    },
                    body.parameters()[0].type_id(),
                    expression.span(),
                );
                let cleanup_scope = CleanupScopeId::from_raw(self.next_cleanup_scope);
                self.next_cleanup_scope = self.next_cleanup_scope.saturating_add(1);
                self.active_cleanups.push(ActiveCleanup {
                    scope: cleanup_scope,
                    action: CleanupAction::FfiBufferBorrow {
                        buffer,
                        region: *region,
                        span: expression.span(),
                    },
                });
                let (captures, declared_effects) = self.lower_scoped_closure(body);
                let result = self.emit(
                    MirInstructionKind::CallScopedBorrow {
                        owner: self.owner,
                        function: body.function(),
                        captures,
                        arguments: vec![pointer, length],
                        region: *region,
                        declared_effects,
                        unwind: MirUnwindAction::Propagate,
                    },
                    expression.type_id(),
                    expression.span(),
                );
                self.active_cleanups
                    .pop()
                    .expect("scoped FFI borrow cleanup was registered");
                self.emit_effect(
                    MirInstructionKind::FfiBufferEndBorrow {
                        buffer,
                        region: *region,
                    },
                    expression.span(),
                );
                return result;
            }
            HirExpressionKind::FfiBytesWithPin {
                bytes,
                body,
                region,
                ..
            } => {
                let bytes = self.lower_expression(bytes);
                let pointer = self.emit(
                    MirInstructionKind::FfiBytesBorrow {
                        bytes,
                        region: *region,
                    },
                    body.parameters()[0].type_id(),
                    expression.span(),
                );
                let cleanup_scope = CleanupScopeId::from_raw(self.next_cleanup_scope);
                self.next_cleanup_scope = self.next_cleanup_scope.saturating_add(1);
                self.active_cleanups.push(ActiveCleanup {
                    scope: cleanup_scope,
                    action: CleanupAction::FfiBytesBorrow {
                        bytes,
                        region: *region,
                        span: expression.span(),
                    },
                });
                let length = self.emit(
                    MirInstructionKind::FfiBytesBorrowLength {
                        bytes,
                        region: *region,
                    },
                    body.parameters()[1].type_id(),
                    expression.span(),
                );
                let (captures, declared_effects) = self.lower_scoped_closure(body);
                let result = self.emit(
                    MirInstructionKind::CallScopedBorrow {
                        owner: self.owner,
                        function: body.function(),
                        captures,
                        arguments: vec![pointer, length],
                        region: *region,
                        declared_effects,
                        unwind: MirUnwindAction::Propagate,
                    },
                    expression.type_id(),
                    expression.span(),
                );
                self.active_cleanups
                    .pop()
                    .expect("scoped FFI byte cleanup was registered");
                self.emit_effect(
                    MirInstructionKind::FfiBytesEndBorrow {
                        bytes,
                        region: *region,
                    },
                    expression.span(),
                );
                return result;
            }
            HirExpressionKind::FfiPointerNone { .. } => MirInstructionKind::FfiPointerNone,
            HirExpressionKind::FfiPointerToOptional { pointer } => {
                MirInstructionKind::FfiPointerToOptional {
                    pointer: self.lower_expression(pointer),
                }
            }
            HirExpressionKind::FfiPointerReadOnly { pointer } => {
                MirInstructionKind::FfiPointerReadOnly {
                    pointer: self.lower_expression(pointer),
                }
            }
            HirExpressionKind::FfiPointerIsPresent { pointer } => {
                MirInstructionKind::FfiPointerIsPresent {
                    pointer: self.lower_expression(pointer),
                }
            }
            HirExpressionKind::FfiPointerRequire {
                pointer,
                result,
                success,
                failure,
            } => MirInstructionKind::FfiPointerRequire {
                pointer: self.lower_expression(pointer),
                result: *result,
                success: *success,
                failure: *failure,
            },
            HirExpressionKind::FfiUnsafeLoad {
                pointer, element, ..
            } => {
                let (layout, _, _) = self.ffi_layout(*element);
                MirInstructionKind::FfiUnsafeLoad {
                    pointer: self.lower_expression(pointer),
                    layout,
                }
            }
            HirExpressionKind::FfiUnsafeStore {
                pointer,
                value,
                element,
                ..
            } => {
                let (layout, _, _) = self.ffi_layout(*element);
                let pointer = self.lower_expression(pointer);
                let value = self.lower_expression(value);
                self.emit_effect(
                    MirInstructionKind::FfiUnsafeStore {
                        pointer,
                        value,
                        layout,
                    },
                    expression.span(),
                );
                MirInstructionKind::NilConstant
            }
            HirExpressionKind::FfiUnsafeAdvance {
                pointer,
                elements,
                element,
                read_only,
                ..
            } => {
                let (layout, _, _) = self.ffi_layout(*element);
                MirInstructionKind::FfiUnsafeAdvance {
                    pointer: self.lower_expression(pointer),
                    elements: self.lower_expression(elements),
                    layout,
                    read_only: *read_only,
                }
            }
            HirExpressionKind::FfiUnsafeCopy {
                source,
                destination,
                count,
                element,
                ..
            } => {
                let (layout, _, _) = self.ffi_layout(*element);
                let source = self.lower_expression(source);
                let destination = self.lower_expression(destination);
                let count = self.lower_expression(count);
                self.emit_effect(
                    MirInstructionKind::FfiUnsafeCopy {
                        source,
                        destination,
                        count,
                        layout,
                    },
                    expression.span(),
                );
                MirInstructionKind::NilConstant
            }
            HirExpressionKind::FfiUnsafeAddress {
                pointer, element, ..
            } => {
                let (layout, _, _) = self.ffi_layout(*element);
                MirInstructionKind::FfiUnsafeAddress {
                    pointer: self.lower_expression(pointer),
                    layout,
                }
            }
            HirExpressionKind::FfiUnsafePointerFromAddress {
                address, element, ..
            } => {
                let (layout, _, _) = self.ffi_layout(*element);
                MirInstructionKind::FfiUnsafePointerFromAddress {
                    address: self.lower_expression(address),
                    layout,
                }
            }
            HirExpressionKind::InterfaceUpcast { value, interface } => {
                let value = self.lower_expression(value);
                MirInstructionKind::InterfaceUpcast {
                    value,
                    interface: *interface,
                }
            }
            HirExpressionKind::CheckedNominalCast {
                value,
                source_interface,
                source_type,
                target_class,
                target_type,
            } => MirInstructionKind::CheckedDowncast {
                value: self.lower_expression(value),
                source_interface: *source_interface,
                source_type: *source_type,
                target_class: *target_class,
                target_type: *target_type,
            },
            HirExpressionKind::ViewCreate {
                kind,
                lender,
                borrow,
            } => MirInstructionKind::ViewCreate {
                kind: mir_view_kind(*kind),
                lender: self.lower_expression(lender),
                lender_provenance: mir_view_lender(borrow.lender()),
                range_unit: mir_view_kind(*kind).range_unit(),
                boundary: mir_view_kind(*kind).boundary_proof(),
                borrow_lifetime: borrow.lifetime(),
            },
            HirExpressionKind::ViewSlice {
                kind,
                view,
                start,
                length,
                parent,
                borrow,
            } => MirInstructionKind::ViewSlice {
                kind: mir_view_kind(*kind),
                view: self.lower_expression(view),
                start: self.lower_expression(start),
                length: self.lower_expression(length),
                lender_provenance: mir_view_lender(borrow.lender()),
                range_unit: mir_view_kind(*kind).range_unit(),
                boundary: mir_view_kind(*kind).boundary_proof(),
                parent_lifetime: match parent.lender() {
                    pop_types::ViewLenderProvenance::Parameter { index }
                        if self
                            .parameters_schema
                            .get(usize::try_from(index).unwrap_or(usize::MAX))
                            .is_some_and(|type_id| self.arena.view_kind(*type_id).is_some()) =>
                    {
                        LifetimeId::from_raw(u32::MAX.saturating_sub(index))
                    }
                    _ => parent.lifetime(),
                },
                borrow_lifetime: borrow.lifetime(),
                bounds_trap: MirViewTrap::BoundsViolation,
            },
            HirExpressionKind::ViewLength { kind, view } => MirInstructionKind::ViewLength {
                kind: mir_view_kind(*kind),
                view: self.lower_expression(view),
            },
            HirExpressionKind::ViewGetByte { view, index } => MirInstructionKind::ViewGetByte {
                view: self.lower_expression(view),
                index: self.lower_expression(index),
            },
            HirExpressionKind::ViewMaterialize {
                kind,
                view,
                allocation_site,
            } => MirInstructionKind::ViewMaterialize {
                kind: mir_view_kind(*kind),
                view: self.lower_expression(view),
                allocation_site: *allocation_site,
            },
            HirExpressionKind::NumericConvert { value, conversion } => {
                let operand = self.lower_expression(value);
                match conversion {
                    NumericConversionKind::IntegerToInteger { source, target } => {
                        MirInstructionKind::ConvertInteger {
                            source: *source,
                            target: *target,
                            operand,
                        }
                    }
                    NumericConversionKind::IntegerToFloat { source, target } => {
                        MirInstructionKind::ConvertIntegerToFloat {
                            source: *source,
                            target: *target,
                            operand,
                        }
                    }
                    NumericConversionKind::FloatToInteger { source, target } => {
                        MirInstructionKind::ConvertFloatToInteger {
                            source: *source,
                            target: *target,
                            operand,
                        }
                    }
                    NumericConversionKind::FloatToFloat { source, target } => {
                        MirInstructionKind::ConvertFloat {
                            source: *source,
                            target: *target,
                            operand,
                        }
                    }
                }
            }
            HirExpressionKind::Field { base, field } => MirInstructionKind::FieldGet {
                base: self.lower_expression(base),
                field: *field,
            },
            HirExpressionKind::ArrayGet { array, index } => {
                let array = self.lower_expression(array);
                let index = self.lower_expression(index);
                MirInstructionKind::ArrayGet { array, index }
            }
            HirExpressionKind::TupleGet { tuple, index } => MirInstructionKind::TupleGet {
                tuple: self.lower_expression(tuple),
                index: *index,
            },
            HirExpressionKind::ArrayLength { array } => MirInstructionKind::ArrayLength {
                array: self.lower_expression(array),
            },
            HirExpressionKind::ArrayGetChecked { array, index } => {
                MirInstructionKind::ArrayGetChecked {
                    array: self.lower_expression(array),
                    index: self.lower_expression(index),
                }
            }
            HirExpressionKind::ArrayFill { array, value } => MirInstructionKind::ArrayFill {
                array: self.lower_expression(array),
                value: self.lower_expression(value),
                element_map: array_element_map(self.arena, array.type_id()),
            },
            HirExpressionKind::ListCreate { capacity } => MirInstructionKind::ListCreate {
                capacity: capacity
                    .as_ref()
                    .map(|capacity| self.lower_expression(capacity)),
                element_map: list_element_map(self.arena, expression.type_id()),
            },
            HirExpressionKind::ListLength { list } => MirInstructionKind::ListLength {
                list: self.lower_expression(list),
            },
            HirExpressionKind::ListGet { list, index } => MirInstructionKind::ListGet {
                list: self.lower_expression(list),
                index: self.lower_expression(index),
            },
            HirExpressionKind::ListGetChecked { list, index } => {
                MirInstructionKind::ListGetChecked {
                    list: self.lower_expression(list),
                    index: self.lower_expression(index),
                }
            }
            HirExpressionKind::ListAdd { list, value } => MirInstructionKind::ListAdd {
                list: self.lower_expression(list),
                value: self.lower_expression(value),
                element_map: list_element_map(self.arena, list.type_id()),
            },
            HirExpressionKind::RangeCreate { first, last, step } => {
                MirInstructionKind::RangeCreate {
                    first: self.lower_expression(first),
                    last: self.lower_expression(last),
                    step: self.lower_expression(step),
                }
            }
            HirExpressionKind::Record { record, fields } => MirInstructionKind::RecordMake {
                record: *record,
                fields: self.lower_fields(fields),
            },
            HirExpressionKind::ClassConstruct { class, fields, .. } => {
                MirInstructionKind::ClassMake {
                    class: *class,
                    fields: self.lower_fields(fields),
                    object_map: self
                        .gc_schema
                        .classes
                        .get(class)
                        .cloned()
                        .expect("verified class construction has a GC schema"),
                }
            }
            HirExpressionKind::RecordUpdate {
                record,
                base,
                fields,
            } => {
                let base = self.lower_expression(base);
                MirInstructionKind::RecordUpdate {
                    record: *record,
                    base,
                    fields: self.lower_fields(fields),
                }
            }
            HirExpressionKind::UnionCase {
                union,
                case,
                arguments,
            } => MirInstructionKind::UnionMake {
                union: *union,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            },
            HirExpressionKind::ResultCase {
                result,
                case,
                arguments,
            } => MirInstructionKind::ResultMake {
                result: *result,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            },
            HirExpressionKind::IterationCase {
                iteration,
                case,
                arguments,
            } => MirInstructionKind::IterationMake {
                iteration: *iteration,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            },
            HirExpressionKind::ErrorCase {
                error,
                case,
                arguments,
            } => MirInstructionKind::ErrorMake {
                error: *error,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            },
        };
        self.emit(kind, expression.type_id(), expression.span())
    }

    fn lower_scoped_closure(
        &mut self,
        closure: &HirClosure,
    ) -> (Vec<MirClosureCapture>, MirEffectSummary) {
        let (lowered, mut nested) = FunctionBuilder::new_closure(
            self.owner,
            closure,
            self.arena,
            self.gc_schema,
            self.reference_effects,
            self.function_effects,
            self.method_effects,
            self.builtin_interface_effects,
            self.ffi_layouts,
        )
        .lower();
        let captures: Vec<_> = closure
            .captures()
            .iter()
            .enumerate()
            .map(|(slot, capture)| {
                let self_reference = matches!(
                    (capture.source(), capture.mode()),
                    (HirCaptureSource::Local(local), HirCaptureMode::Cell)
                        if !self.local_cells.contains_key(&local)
                );
                let value = if self_reference {
                    ValueId::from_raw(u32::MAX)
                } else {
                    self.lower_capture_source(
                        capture.source(),
                        capture.mode(),
                        capture.type_id(),
                        closure.span(),
                    )
                };
                MirClosureCapture {
                    capture: capture.capture(),
                    binding: capture.binding(),
                    slot: u32::try_from(slot).unwrap_or(u32::MAX),
                    value,
                    self_reference,
                    type_id: capture.type_id(),
                    mode: match capture.mode() {
                        HirCaptureMode::Value => MirCaptureMode::Value,
                        HirCaptureMode::Cell => MirCaptureMode::Cell,
                    },
                }
            })
            .collect();
        let mut nested_function = MirNestedFunction {
            owner: self.owner,
            function: closure.function(),
            is_async: closure.is_async(),
            captures: closure
                .captures()
                .iter()
                .enumerate()
                .map(|(slot, capture)| MirCapture {
                    capture: capture.capture(),
                    binding: capture.binding(),
                    slot: u32::try_from(slot).unwrap_or(u32::MAX),
                    type_id: capture.type_id(),
                    mode: match capture.mode() {
                        HirCaptureMode::Value => MirCaptureMode::Value,
                        HirCaptureMode::Cell => MirCaptureMode::Cell,
                    },
                })
                .collect(),
            parameters: lowered.parameters,
            results: lowered.results,
            effects: lowered.effects,
            effects_explicit: lowered.effects_explicit,
            blocks: lowered.blocks,
        };
        let mut adapter = nested_function.transformation_adapter();
        while recompute_function_effects(&mut adapter, self.function_effects, self.method_effects) {
        }
        nested_function.apply_transformation(adapter);
        let declared_effects = nested_function.effects();
        self.nested_functions.push(nested_function);
        self.nested_functions.append(&mut nested);
        (captures, declared_effects)
    }

    fn lower_closure(&mut self, closure: &HirClosure, closure_type: TypeId) -> ValueId {
        let (captures, _) = self.lower_scoped_closure(closure);
        let object_map = closure_environment_object_map(self.arena, &captures);
        self.emit(
            MirInstructionKind::ClosureEnvironmentAllocate {
                owner: self.owner,
                function: closure.function(),
                captures,
                object_map,
            },
            closure_type,
            closure.span(),
        )
    }

    fn ffi_layout(&self, element: TypeId) -> (FfiAbiLayoutId, u64, u64) {
        let layout = self
            .ffi_layouts
            .entries()
            .iter()
            .find(|layout| layout.element() == element)
            .expect("verified source FFI buffer type has one target layout");
        (layout.id(), layout.size(), layout.alignment())
    }

    fn registered_callback_type(&self, callback_type: TypeId) -> TypeId {
        self.arena
            .find(&SemanticType::Builtin {
                definition: pop_types::FFI_REGISTERED_CALLBACK_TYPE_ID,
                arguments: vec![callback_type],
            })
            .expect("verified FFI callback HIR has an interned registration type")
    }

    fn ffi_callback_signature(
        &self,
        callback_type: TypeId,
        contract: &pop_types::FfiCallbackBindingContract,
    ) -> MirFfiCallbackSignature {
        let abi = MirFfiCallbackAbi::from(contract.callback_abi());
        let foreign_abi = match abi {
            MirFfiCallbackAbi::C => pop_types::ForeignAbi::C,
            MirFfiCallbackAbi::System => pop_types::ForeignAbi::System,
        };
        let SemanticType::Function {
            parameters,
            results,
            ..
        } = self
            .arena
            .get(callback_type)
            .expect("verified callback type exists")
        else {
            unreachable!("verified callback type is a function")
        };
        let layout = |type_id: TypeId| {
            matches!(self.arena.get(type_id), Some(SemanticType::Record(_))).then(|| {
                self.ffi_layouts
                    .entries()
                    .iter()
                    .find(|entry| entry.element() == type_id && entry.abi() == foreign_abi)
                    .expect("verified callback record has an exact target ABI layout")
                    .id()
            })
        };
        let fingerprint =
            MirFfiCallbackFingerprint::from_lower_hex(contract.signature_fingerprint())
                .expect("verified callback binding has an exact lowercase SHA-256 fingerprint");
        MirFfiCallbackSignature::new(
            callback_type,
            abi,
            parameters.iter().copied().map(layout).collect(),
            results.first().copied().and_then(layout),
            fingerprint,
        )
    }

    fn result_definition(&self, result_type: TypeId) -> pop_foundation::BuiltinTypeId {
        match self.arena.get(result_type) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if arguments.len() == 2 => *definition,
            _ => unreachable!("verified callback operation has a Result type"),
        }
    }

    fn lower_capture_source(
        &mut self,
        source: HirCaptureSource,
        mode: HirCaptureMode,
        type_id: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        match (source, mode) {
            (HirCaptureSource::Local(local), HirCaptureMode::Cell) => self.local_cells[&local],
            (HirCaptureSource::Parameter(parameter), HirCaptureMode::Cell) => {
                self.parameter_cells[&parameter]
            }
            (HirCaptureSource::Capture(capture), HirCaptureMode::Cell) => {
                let schema = self.capture_schema[&capture];
                self.emit(
                    MirInstructionKind::CaptureCellReference {
                        capture,
                        slot: schema.slot(),
                    },
                    type_id,
                    span,
                )
            }
            (HirCaptureSource::Local(local), HirCaptureMode::Value) => {
                if let Some(cell) = self.local_cells.get(&local).copied() {
                    self.emit(MirInstructionKind::CaptureCellLoad { cell }, type_id, span)
                } else {
                    self.locals[&local]
                }
            }
            (HirCaptureSource::Parameter(parameter), HirCaptureMode::Value) => {
                if let Some(cell) = self.parameter_cells.get(&parameter).copied() {
                    self.emit(MirInstructionKind::CaptureCellLoad { cell }, type_id, span)
                } else {
                    self.parameters[&parameter]
                }
            }
            (HirCaptureSource::Capture(capture), HirCaptureMode::Value) => {
                let schema = self.capture_schema[&capture];
                self.emit(
                    MirInstructionKind::CaptureLoad {
                        capture,
                        slot: schema.slot(),
                        mode: schema.mode(),
                    },
                    type_id,
                    span,
                )
            }
        }
    }

    fn lower_binary_expression(
        &mut self,
        expression: &HirExpression,
        operator: TypedBinaryOperator,
        left: &HirExpression,
        right: &HirExpression,
    ) -> ValueId {
        if matches!(operator, TypedBinaryOperator::And | TypedBinaryOperator::Or) {
            return self.lower_short_circuit(
                operator,
                left,
                right,
                expression.type_id(),
                expression.span(),
            );
        }
        if matches!(
            operator,
            TypedBinaryOperator::Equal | TypedBinaryOperator::NotEqual
        ) {
            let optional = if matches!(right.kind(), HirExpressionKind::Nil)
                && optional_inner_type(self.arena, left.type_id()).is_some()
            {
                Some(left)
            } else if matches!(left.kind(), HirExpressionKind::Nil)
                && optional_inner_type(self.arena, right.type_id()).is_some()
            {
                Some(right)
            } else {
                None
            };
            if let Some(optional) = optional {
                let optional = self.lower_expression(optional);
                let present = self.emit(
                    MirInstructionKind::OptionalIsPresent { optional },
                    expression.type_id(),
                    expression.span(),
                );
                return if operator == TypedBinaryOperator::Equal {
                    self.emit(
                        MirInstructionKind::BooleanNot { operand: present },
                        expression.type_id(),
                        expression.span(),
                    )
                } else {
                    present
                };
            }
        }
        let operand_type = left.type_id();
        let left = self.lower_expression(left);
        let right = self.lower_expression(right);
        self.emit(
            lower_binary(self.arena, operator, operand_type, left, right),
            expression.type_id(),
            expression.span(),
        )
    }

    fn lower_optional_default(
        &mut self,
        optional: &HirExpression,
        fallback: &HirExpression,
        result_type: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let optional = self.lower_expression(optional);
        let present = self.emit(
            MirInstructionKind::OptionalIsPresent { optional },
            self.arena.source_type("Boolean").expect("Boolean"),
            span,
        );
        let present_block = self.new_block();
        let fallback_block = self.new_block();
        let (join_block, result) = self.new_block_with_argument(result_type, span);
        self.terminate(MirTerminator::ConditionalBranch {
            condition: present,
            when_true: present_block,
            when_false: fallback_block,
        });
        self.current = present_block;
        let value = self.emit(
            MirInstructionKind::OptionalGet { optional },
            result_type,
            span,
        );
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![value],
        });
        self.current = fallback_block;
        let fallback = self.lower_expression(fallback);
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![fallback],
        });
        self.current = join_block;
        result
    }

    fn lower_optional_propagate(
        &mut self,
        optional: &HirExpression,
        enclosing_result: TypeId,
        result_type: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let optional = self.lower_expression(optional);
        let present = self.emit(
            MirInstructionKind::OptionalIsPresent { optional },
            self.arena.source_type("Boolean").expect("Boolean"),
            span,
        );
        let present_block = self.new_block();
        let absent_block = self.new_block();
        let (join_block, result) = self.new_block_with_argument(result_type, span);
        self.terminate(MirTerminator::ConditionalBranch {
            condition: present,
            when_true: present_block,
            when_false: absent_block,
        });
        self.current = absent_block;
        let nil = self.emit(MirInstructionKind::NilConstant, enclosing_result, span);
        self.emit_cleanups_to(0, MirCleanupExitReason::Return);
        self.terminate(MirTerminator::Return { values: vec![nil] });
        self.current = present_block;
        let value = self.emit(
            MirInstructionKind::OptionalGet { optional },
            result_type,
            span,
        );
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![value],
        });
        self.current = join_block;
        result
    }

    fn lower_result_propagate(
        &mut self,
        result: &HirExpression,
        result_definition: pop_foundation::BuiltinTypeId,
        success_type: TypeId,
        error_type: TypeId,
        enclosing_result: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let result = self.lower_expression(result);
        let is_ok = self.emit(
            MirInstructionKind::ResultIsOk {
                result,
                definition: result_definition,
            },
            self.arena.source_type("Boolean").expect("Boolean"),
            span,
        );
        let success_block = self.new_block();
        let error_block = self.new_block();
        let (join_block, success) = self.new_block_with_argument(success_type, span);
        self.terminate(MirTerminator::ConditionalBranch {
            condition: is_ok,
            when_true: success_block,
            when_false: error_block,
        });

        self.current = error_block;
        let error = self.emit(
            MirInstructionKind::ResultGetError {
                result,
                definition: result_definition,
            },
            error_type,
            span,
        );
        let propagated = self.emit(
            MirInstructionKind::ResultMake {
                result: result_definition,
                case: ResultCaseId::from_raw(1),
                arguments: vec![error],
            },
            enclosing_result,
            span,
        );
        self.emit_cleanups_to(0, MirCleanupExitReason::ResultFailure);
        self.terminate(MirTerminator::Return {
            values: vec![propagated],
        });

        self.current = success_block;
        let value = self.emit(
            MirInstructionKind::ResultGetOk {
                result,
                definition: result_definition,
            },
            success_type,
            span,
        );
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![value],
        });
        self.current = join_block;
        success
    }

    fn lower_compound_value(
        &mut self,
        operator: TypedCompoundOperator,
        value_type: TypeId,
        left: ValueId,
        right: ValueId,
        span: SourceSpan,
    ) -> ValueId {
        let kind = match operator {
            TypedCompoundOperator::Concat => MirInstructionKind::StringConcat { left, right },
            operator => lower_binary(
                self.arena,
                match operator {
                    TypedCompoundOperator::Add => TypedBinaryOperator::Add,
                    TypedCompoundOperator::Subtract => TypedBinaryOperator::Subtract,
                    TypedCompoundOperator::Multiply => TypedBinaryOperator::Multiply,
                    TypedCompoundOperator::Divide => TypedBinaryOperator::Divide,
                    TypedCompoundOperator::Remainder => TypedBinaryOperator::Remainder,
                    TypedCompoundOperator::Concat => unreachable!(),
                },
                value_type,
                left,
                right,
            ),
        };
        self.emit(kind, value_type, span)
    }

    fn lower_short_circuit(
        &mut self,
        operator: TypedBinaryOperator,
        left: &HirExpression,
        right: &HirExpression,
        result_type: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let left = self.lower_expression(left);
        let right_block = self.new_block();
        let short_block = self.new_block();
        let (join_block, result) = self.new_block_with_argument(result_type, span);
        let (when_true, when_false, short_value) = match operator {
            TypedBinaryOperator::And => (right_block, short_block, false),
            TypedBinaryOperator::Or => (short_block, right_block, true),
            _ => unreachable!("short-circuit lowering accepts only logical operators"),
        };
        self.terminate(MirTerminator::ConditionalBranch {
            condition: left,
            when_true,
            when_false,
        });

        self.current = short_block;
        let short_value = self.emit(
            MirInstructionKind::BooleanConstant(short_value),
            result_type,
            span,
        );
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![short_value],
        });

        self.current = right_block;
        let right = self.lower_expression(right);
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![right],
        });
        self.current = join_block;
        result
    }

    fn lower_conditional_expression(
        &mut self,
        condition: &HirExpression,
        when_true: &HirExpression,
        when_false: &HirExpression,
        result_type: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let condition = self.lower_expression(condition);
        let true_block = self.new_block();
        let false_block = self.new_block();
        let (join_block, result) = self.new_block_with_argument(result_type, span);
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: true_block,
            when_false: false_block,
        });

        self.current = true_block;
        let when_true = self.lower_expression(when_true);
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![when_true],
        });

        self.current = false_block;
        let when_false = self.lower_expression(when_false);
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![when_false],
        });

        self.current = join_block;
        result
    }

    fn lower_fields(&mut self, fields: &[HirFieldValue]) -> Vec<(FieldId, ValueId)> {
        fields
            .iter()
            .map(|field| (field.field(), self.lower_expression(field.value())))
            .collect()
    }

    fn lower_task_create(
        &mut self,
        dispatch: &HirCallDispatch,
        arguments: &[HirExpression],
        task_type: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let completion_type = match self.arena.get(task_type) {
            Some(SemanticType::Builtin { arguments, .. }) if arguments.len() == 1 => arguments[0],
            _ => unreachable!("verified async calls produce exact Task<T> values"),
        };
        let dispatch = match dispatch {
            HirCallDispatch::Direct { function } => MirTaskDispatch::Direct(*function),
            HirCallDispatch::Referenced { function } => MirTaskDispatch::Referenced(*function),
            HirCallDispatch::Indirect { callee } => {
                MirTaskDispatch::Indirect(self.lower_expression(callee))
            }
            HirCallDispatch::Standard { .. }
            | HirCallDispatch::DirectMethod { .. }
            | HirCallDispatch::InterfaceMethod { .. }
            | HirCallDispatch::BuiltinInterfaceMethod { .. } => {
                unreachable!("verified async calls use function dispatch")
            }
        };
        let arguments = arguments
            .iter()
            .map(|argument| self.lower_expression(argument))
            .collect::<Vec<_>>();
        let argument_types = arguments
            .iter()
            .map(|argument| self.value_type(*argument))
            .collect::<Vec<_>>();
        let object_map = task_object_map(&dispatch, &argument_types, completion_type, self.arena);
        self.emit(
            MirInstructionKind::TaskCreate {
                dispatch,
                arguments,
                completion_type,
                object_map,
            },
            task_type,
            span,
        )
    }

    fn lower_await(
        &mut self,
        task: &HirExpression,
        result_type: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let task = self.lower_expression(task);
        let state = self.live_state(span);
        let mut frame_values = vec![task];
        frame_values.extend(self.state_values(&state));
        frame_values.extend(self.parameter_cells.values().copied());
        frame_values.extend(self.local_cells.values().copied());
        let mut seen = BTreeSet::new();
        frame_values.retain(|value| seen.insert(*value));

        let coroutine_state = CoroutineStateId::from_raw(self.next_coroutine_state);
        self.next_coroutine_state = self.next_coroutine_state.saturating_add(1);
        let safe_point = SafePointId::new(self.next_suspend_safe_point);
        self.next_suspend_safe_point = self.next_suspend_safe_point.saturating_add(1);
        let slots = frame_values
            .into_iter()
            .map(|value| MirFrameSlot {
                value,
                type_id: self.value_type(value),
            })
            .collect::<Vec<_>>();
        let root_slots = slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                if is_managed_reference_type_id(slot.type_id, Some(self.arena)) {
                    Some(RootSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
                } else {
                    None
                }
            })
            .collect();
        let stack_map = StackMap::new(safe_point, root_slots)
            .expect("coroutine frame slots produce a canonical stack map");
        let live_frame = MirLiveFrame {
            state: coroutine_state,
            slots,
            stack_map,
        };

        let suspended = self.current;
        let cancellation = self.build_suspend_exit(
            MirCleanupExitReason::Cancellation,
            MirTerminator::ContinueUnwind(pop_runtime_interface::UnwindReason::Cancellation),
        );
        let unwind = if self.active_cleanups.is_empty() {
            MirUnwindAction::Propagate
        } else {
            MirUnwindAction::Cleanup(
                self.build_suspend_exit(MirCleanupExitReason::Unwind, MirTerminator::ResumeUnwind),
            )
        };
        let (resume, result) = self.new_block_with_argument(result_type, span);
        self.current = suspended;
        self.terminate(MirTerminator::Suspend {
            operation: MirSuspendOperation::Task { task, result_type },
            resume,
            cancellation,
            cancellation_mode: if self.current_cleanup.is_some() {
                MirCancellationMode::Masked
            } else {
                MirCancellationMode::Observe
            },
            unwind,
            safe_point,
            live_frame,
        });
        self.current = resume;
        result
    }

    fn build_suspend_exit(
        &mut self,
        reason: MirCleanupExitReason,
        terminator: MirTerminator,
    ) -> BlockId {
        let suspended = self.current;
        let registered = self.active_cleanups.clone();
        let scope = registered.last().map_or_else(
            || {
                let scope = CleanupScopeId::from_raw(self.next_cleanup_scope);
                self.next_cleanup_scope = self.next_cleanup_scope.saturating_add(1);
                scope
            },
            |cleanup| cleanup.scope,
        );
        let entry = self.new_cleanup_block(MirCleanupBlock { scope, reason });
        self.current = entry;
        self.emit_cleanups_to(0, reason);
        self.terminate(terminator);
        self.active_cleanups = registered;
        self.current = suspended;
        entry
    }

    fn lower_call(
        &mut self,
        dispatch: &HirCallDispatch,
        arguments: &[HirExpression],
    ) -> MirInstructionKind {
        match dispatch {
            HirCallDispatch::Standard { function } => MirInstructionKind::CallStandard {
                function: *function,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: MirEffectSummary::empty().with(MirEffect::AmbientIo),
            },
            HirCallDispatch::Direct { function } => MirInstructionKind::CallDirect {
                function: *function,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                lifetime_summary: pop_types::CallableLifetimeSummary::conservative(
                    arguments.len(),
                    1,
                ),
                view_result: None,
                declared_effects: self
                    .function_effects
                    .get(function)
                    .copied()
                    .unwrap_or_default(),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::Referenced { function } => MirInstructionKind::CallReferenced {
                function: *function,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                lifetime_summary: pop_types::CallableLifetimeSummary::conservative(
                    arguments.len(),
                    1,
                ),
                view_result: None,
                declared_effects: self
                    .reference_effects
                    .get(function)
                    .copied()
                    .unwrap_or_default(),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::DirectMethod { method } => MirInstructionKind::CallDirectMethod {
                method: *method,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: self.method_effects.get(method).copied().unwrap_or_default(),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::InterfaceMethod {
                interface,
                method,
                slot,
                effects,
            } => MirInstructionKind::CallInterface {
                interface: *interface,
                method: *method,
                slot: *slot,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: lower_effect_summary(*effects),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::BuiltinInterfaceMethod {
                interface,
                method,
                effects,
            } => MirInstructionKind::CallBuiltinInterface {
                interface: *interface,
                method: *method,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: lower_effect_summary(*effects),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::Indirect { callee } => {
                let declared_effects = match self.arena.get(callee.type_id()) {
                    Some(SemanticType::Function { effects, .. }) => lower_effect_summary(*effects),
                    _ => MirEffectSummary::empty(),
                };
                let callee = self.lower_expression(callee);
                MirInstructionKind::CallIndirect {
                    callee,
                    arguments: arguments
                        .iter()
                        .map(|argument| self.lower_expression(argument))
                        .collect(),
                    declared_effects,
                    unwind: MirUnwindAction::Propagate,
                }
            }
        }
    }

    fn attach_cleanup_unwind(
        &mut self,
        mut instruction: MirInstructionKind,
    ) -> (MirInstructionKind, MirUnwindAction) {
        if self.active_cleanups.is_empty()
            || !local_instruction_effects(&instruction).contains(MirEffect::MayUnwind)
        {
            return (instruction, MirUnwindAction::Propagate);
        }
        let call_block = self.current;
        let registered = std::mem::take(&mut self.active_cleanups);
        let cleanups = registered.clone();
        let mut cleanup_entry = None;
        for cleanup in cleanups.into_iter().rev() {
            let block = self.new_cleanup_block(MirCleanupBlock {
                scope: cleanup.scope,
                reason: MirCleanupExitReason::Unwind,
            });
            if cleanup_entry.is_none() {
                cleanup_entry = Some(block);
            } else {
                self.branch_if_open(block);
            }
            self.current = block;
            let previous_cleanup = self.current_cleanup.replace(MirCleanupBlock {
                scope: cleanup.scope,
                reason: MirCleanupExitReason::Unwind,
            });
            self.lower_cleanup_action(cleanup.action);
            self.current_cleanup = previous_cleanup;
        }
        self.terminate(MirTerminator::ResumeUnwind);
        self.current = call_block;
        self.active_cleanups = registered;
        let cleanup = cleanup_entry.expect("active cleanup set is nonempty");
        let unwind = match &mut instruction {
            MirInstructionKind::CallDirect { unwind, .. }
            | MirInstructionKind::CallForeign { unwind, .. }
            | MirInstructionKind::CallReferenced { unwind, .. }
            | MirInstructionKind::CallDirectMethod { unwind, .. }
            | MirInstructionKind::CallInterface { unwind, .. }
            | MirInstructionKind::CallBuiltinInterface { unwind, .. }
            | MirInstructionKind::CallIndirect { unwind, .. } => Some(unwind),
            MirInstructionKind::CallScopedBorrow { unwind, .. }
            | MirInstructionKind::CallCallbackPair { unwind, .. } => Some(unwind),
            _ => None,
        };
        if let Some(unwind) = unwind {
            *unwind = MirUnwindAction::Cleanup(cleanup);
            (instruction, MirUnwindAction::Propagate)
        } else {
            (instruction, MirUnwindAction::Cleanup(cleanup))
        }
    }

    fn lower_table_entries(&mut self, entries: &[HirTableEntry]) -> Vec<(ValueId, ValueId)> {
        let mut lowered = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = self.lower_expression(entry.key());
            let value = self.lower_expression(entry.value());
            lowered.push((key, value));
        }
        lowered
    }

    fn lower_cleanup_action(&mut self, action: CleanupAction<'hir>) {
        match action {
            CleanupAction::Statements(statements) => self.lower_statements(statements),
            CleanupAction::FfiBufferBorrow {
                buffer,
                region,
                span,
            } => self.emit_effect(
                MirInstructionKind::FfiBufferEndBorrow { buffer, region },
                span,
            ),
            CleanupAction::FfiBytesBorrow {
                bytes,
                region,
                span,
            } => self.emit_effect(
                MirInstructionKind::FfiBytesEndBorrow { bytes, region },
                span,
            ),
            CleanupAction::FfiCallback {
                callback,
                region,
                span,
            } => self.emit_effect(
                MirInstructionKind::FfiCallbackCloseScoped { callback, region },
                span,
            ),
        }
    }

    fn emit(&mut self, kind: MirInstructionKind, type_id: TypeId, span: SourceSpan) -> ValueId {
        let (kind, unwind) = self.attach_cleanup_unwind(kind);
        let value = ValueId::from_raw(self.next_value);
        self.next_value = self.next_value.saturating_add(1);
        let effects = local_instruction_effects(&kind);
        self.current_block_mut().instructions.push(MirInstruction {
            result: value,
            result_type: Some(type_id),
            kind,
            effects,
            effects_explicit: false,
            unwind,
            span,
        });
        value
    }

    fn emit_effect(&mut self, kind: MirInstructionKind, span: SourceSpan) {
        let (kind, unwind) = self.attach_cleanup_unwind(kind);
        let instruction = ValueId::from_raw(self.next_value);
        self.next_value = self.next_value.saturating_add(1);
        let effects = local_instruction_effects(&kind);
        self.current_block_mut().instructions.push(MirInstruction {
            result: instruction,
            result_type: None,
            kind,
            effects,
            effects_explicit: false,
            unwind,
            span,
        });
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId::from_raw(u32::try_from(self.blocks.len()).unwrap_or(u32::MAX));
        self.blocks.push(BuildingBlock {
            cleanup: self.current_cleanup,
            arguments: Vec::new(),
            instructions: Vec::new(),
            terminator: MirTerminator::Missing,
        });
        id
    }

    fn new_cleanup_block(&mut self, cleanup: MirCleanupBlock) -> BlockId {
        let block = self.new_block();
        self.blocks[block.raw() as usize].cleanup = Some(cleanup);
        block
    }

    fn new_block_with_argument(&mut self, type_id: TypeId, span: SourceSpan) -> (BlockId, ValueId) {
        let block = self.new_block();
        let value = ValueId::from_raw(self.next_value);
        self.next_value = self.next_value.saturating_add(1);
        self.blocks[block.raw() as usize]
            .arguments
            .push(MirBlockArgument {
                value,
                type_id,
                span,
            });
        (block, value)
    }

    fn new_block_with_arguments(
        &mut self,
        arguments: &[(TypeId, SourceSpan)],
    ) -> (BlockId, Vec<ValueId>) {
        let block = self.new_block();
        let mut values = Vec::with_capacity(arguments.len());
        for (type_id, span) in arguments {
            let value = ValueId::from_raw(self.next_value);
            self.next_value = self.next_value.saturating_add(1);
            self.blocks[block.raw() as usize]
                .arguments
                .push(MirBlockArgument {
                    value,
                    type_id: *type_id,
                    span: *span,
                });
            values.push(value);
        }
        (block, values)
    }

    fn live_state(&self, span: SourceSpan) -> LiveState {
        let parameters = self
            .parameters
            .keys()
            .filter(|parameter| !self.parameter_cells.contains_key(parameter))
            .copied()
            .collect::<Vec<_>>();
        let locals = self
            .locals
            .keys()
            .filter(|local| !self.local_cells.contains_key(local))
            .copied()
            .collect::<Vec<_>>();
        let specs = parameters
            .iter()
            .map(|parameter| self.parameters[parameter])
            .chain(locals.iter().map(|local| self.locals[local]))
            .map(|value| (self.value_type(value), span))
            .collect();
        LiveState {
            parameters,
            locals,
            specs,
        }
    }

    fn state_values(&self, state: &LiveState) -> Vec<ValueId> {
        state
            .parameters
            .iter()
            .map(|parameter| self.parameters[parameter])
            .chain(state.locals.iter().map(|local| self.locals[local]))
            .collect()
    }

    fn install_state(&mut self, state: &LiveState, values: &[ValueId]) {
        let parameter_count = state.parameters.len();
        for (parameter, value) in state.parameters.iter().zip(values.iter().copied()) {
            self.parameters.insert(*parameter, value);
        }
        for (local, value) in state
            .locals
            .iter()
            .zip(values[parameter_count..].iter().copied())
        {
            self.locals.insert(*local, value);
        }
    }

    fn value_type(&self, value: ValueId) -> TypeId {
        self.blocks
            .iter()
            .flat_map(|block| block.arguments.iter())
            .find(|argument| argument.value == value)
            .map(|argument| argument.type_id)
            .or_else(|| {
                self.blocks
                    .iter()
                    .flat_map(|block| block.instructions.iter())
                    .find(|instruction| instruction.result == value)
                    .and_then(|instruction| instruction.result_type)
            })
            .expect("lowered live state always has a statically proven MIR type")
    }

    fn current_block(&self) -> &BuildingBlock {
        &self.blocks[self.current.raw() as usize]
    }

    fn current_block_mut(&mut self) -> &mut BuildingBlock {
        &mut self.blocks[self.current.raw() as usize]
    }

    fn terminate(&mut self, terminator: MirTerminator) {
        self.current_block_mut().terminator = terminator;
    }

    fn branch_if_open(&mut self, target: BlockId) {
        if matches!(self.current_block().terminator, MirTerminator::Missing) {
            self.terminate(MirTerminator::Branch {
                target,
                arguments: Vec::new(),
            });
        }
    }

    fn branch_with_arguments_if_open(&mut self, target: BlockId, arguments: Vec<ValueId>) {
        if matches!(self.current_block().terminator, MirTerminator::Missing) {
            self.terminate(MirTerminator::Branch { target, arguments });
        }
    }

    fn branch_with_state_if_open(&mut self, target: BlockId, state: &LiveState) {
        self.branch_with_arguments_if_open(target, self.state_values(state));
    }
}

const fn mir_view_kind(kind: pop_types::ViewKind) -> MirViewKind {
    match kind {
        pop_types::ViewKind::Bytes => MirViewKind::Bytes,
        pop_types::ViewKind::Text => MirViewKind::Text,
    }
}

const fn mir_view_lender(lender: pop_types::ViewLenderProvenance) -> MirViewLender {
    match lender {
        pop_types::ViewLenderProvenance::Allocation { site } => MirViewLender::Allocation { site },
        pop_types::ViewLenderProvenance::Parameter { index } => MirViewLender::Parameter { index },
        pop_types::ViewLenderProvenance::Constant { fingerprint } => {
            MirViewLender::Constant { fingerprint }
        }
    }
}

fn lower_binary(
    arena: &TypeArena,
    operator: TypedBinaryOperator,
    operand_type: TypeId,
    left: ValueId,
    right: ValueId,
) -> MirInstructionKind {
    match operator {
        TypedBinaryOperator::Or => MirInstructionKind::BooleanOr { left, right },
        TypedBinaryOperator::And => MirInstructionKind::BooleanAnd { left, right },
        TypedBinaryOperator::Equal => MirInstructionKind::CompareEqual { left, right },
        TypedBinaryOperator::NotEqual => MirInstructionKind::CompareNotEqual { left, right },
        TypedBinaryOperator::LessThan => {
            if let Some(kind) = integer_kind(arena, operand_type) {
                MirInstructionKind::CompareIntegerLess { kind, left, right }
            } else {
                MirInstructionKind::CompareFloatLess {
                    kind: float_kind(arena, operand_type)
                        .expect("typed comparison has numeric operands"),
                    left,
                    right,
                }
            }
        }
        TypedBinaryOperator::LessThanOrEqual => {
            if let Some(kind) = integer_kind(arena, operand_type) {
                MirInstructionKind::CompareIntegerLessOrEqual { kind, left, right }
            } else {
                MirInstructionKind::CompareFloatLessOrEqual {
                    kind: float_kind(arena, operand_type)
                        .expect("typed comparison has numeric operands"),
                    left,
                    right,
                }
            }
        }
        TypedBinaryOperator::GreaterThan => {
            if let Some(kind) = integer_kind(arena, operand_type) {
                MirInstructionKind::CompareIntegerGreater { kind, left, right }
            } else {
                MirInstructionKind::CompareFloatGreater {
                    kind: float_kind(arena, operand_type)
                        .expect("typed comparison has numeric operands"),
                    left,
                    right,
                }
            }
        }
        TypedBinaryOperator::GreaterThanOrEqual => {
            if let Some(kind) = integer_kind(arena, operand_type) {
                MirInstructionKind::CompareIntegerGreaterOrEqual { kind, left, right }
            } else {
                MirInstructionKind::CompareFloatGreaterOrEqual {
                    kind: float_kind(arena, operand_type)
                        .expect("typed comparison has numeric operands"),
                    left,
                    right,
                }
            }
        }
        TypedBinaryOperator::Add => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerAdd { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatAdd { kind, left, right },
        ),
        TypedBinaryOperator::Subtract => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerSubtract { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatSubtract { kind, left, right },
        ),
        TypedBinaryOperator::Multiply => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerMultiply { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatMultiply { kind, left, right },
        ),
        TypedBinaryOperator::Divide => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerDivide { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatDivide { kind, left, right },
        ),
        TypedBinaryOperator::Remainder => MirInstructionKind::CheckedIntegerRemainder {
            kind: integer_kind(arena, operand_type).expect("typed remainder has integer operands"),
            left,
            right,
        },
    }
}

fn optional_inner_type(arena: &TypeArena, optional: TypeId) -> Option<TypeId> {
    let nil = arena.source_type("nil")?;
    let SemanticType::Union(members) = arena.get(optional)? else {
        return None;
    };
    if !members.contains(&nil) {
        return None;
    }
    let present = members
        .iter()
        .copied()
        .filter(|member| *member != nil)
        .collect::<Vec<_>>();
    match present.as_slice() {
        [inner] => Some(*inner),
        [] => None,
        _ => arena.find(&SemanticType::Union(present)),
    }
}

fn numeric_binary(
    arena: &TypeArena,
    operand_type: TypeId,
    left: ValueId,
    right: ValueId,
    integer: impl FnOnce(IntegerKind, ValueId, ValueId) -> MirInstructionKind,
    float: impl FnOnce(FloatKind, ValueId, ValueId) -> MirInstructionKind,
) -> MirInstructionKind {
    if let Some(kind) = integer_kind(arena, operand_type) {
        integer(kind, left, right)
    } else {
        float(
            float_kind(arena, operand_type).expect("typed arithmetic has numeric operands"),
            left,
            right,
        )
    }
}

fn integer_kind(arena: &TypeArena, type_id: TypeId) -> Option<IntegerKind> {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Some(*kind),
        _ => None,
    }
}

fn float_kind(arena: &TypeArena, type_id: TypeId) -> Option<FloatKind> {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => Some(FloatKind::Float32),
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => Some(FloatKind::Float64),
        _ => None,
    }
}

pub(crate) fn array_element_map(arena: &TypeArena, type_id: TypeId) -> ArrayElementMap {
    match arena.get(type_id) {
        Some(SemanticType::Array(element))
            if is_managed_reference_type_id(*element, Some(arena)) =>
        {
            ArrayElementMap::ManagedReference
        }
        _ => ArrayElementMap::Scalar,
    }
}

pub(crate) fn list_element_map(arena: &TypeArena, type_id: TypeId) -> ArrayElementMap {
    let list = pop_types::embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol())
        .map(|protocol| protocol.list());
    match arena.get(type_id) {
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) if Some(*definition) == list && arguments.len() == 1 => element_map(arena, arguments[0]),
        _ => ArrayElementMap::Scalar,
    }
}

fn ffi_buffer_type_element(arena: &TypeArena, type_id: TypeId) -> Option<TypeId> {
    match arena.get(type_id)? {
        SemanticType::Builtin {
            definition,
            arguments,
        } if *definition == pop_types::FFI_BUFFER_TYPE_ID && arguments.len() == 1 => {
            Some(arguments[0])
        }
        _ => None,
    }
}

pub(crate) fn table_element_maps(
    arena: &TypeArena,
    type_id: TypeId,
) -> (ArrayElementMap, ArrayElementMap) {
    match arena.get(type_id) {
        Some(SemanticType::Table { key, value }) => {
            (element_map(arena, *key), element_map(arena, *value))
        }
        _ => (ArrayElementMap::Scalar, ArrayElementMap::Scalar),
    }
}

fn element_map(arena: &TypeArena, type_id: TypeId) -> ArrayElementMap {
    if is_managed_reference_type_id(type_id, Some(arena)) {
        ArrayElementMap::ManagedReference
    } else {
        ArrayElementMap::Scalar
    }
}

fn capture_cell_object_map(arena: &TypeArena, value_type: TypeId) -> ObjectMap {
    let references = is_managed_reference_type_id(value_type, Some(arena))
        .then(|| ObjectSlot::new(0))
        .into_iter()
        .collect();
    ObjectMap::new(1, references).expect("one-slot capture cell map is canonical")
}

fn closure_environment_object_map(arena: &TypeArena, captures: &[MirClosureCapture]) -> ObjectMap {
    let references = captures
        .iter()
        .filter(|capture| {
            capture.mode == MirCaptureMode::Cell
                || is_managed_reference_type_id(capture.type_id, Some(arena))
        })
        .map(|capture| ObjectSlot::new(capture.slot))
        .collect();
    ObjectMap::new(
        u32::try_from(captures.len()).unwrap_or(u32::MAX),
        references,
    )
    .expect("closure captures form a valid logical object map")
}

#[must_use]
pub fn is_managed_reference_type_id(type_id: TypeId, arena: Option<&TypeArena>) -> bool {
    let Some(arena) = arena else {
        return false;
    };
    match arena.get(type_id) {
        Some(SemanticType::Builtin { definition, .. }) => {
            !pop_types::is_ffi_abi_builtin_type(*definition)
                && *definition != pop_types::BYTES_VIEW_TYPE_ID
                && *definition != pop_types::TEXT_VIEW_TYPE_ID
                && *definition != pop_types::FFI_NULL_POINTER_ERROR_TYPE_ID
                && *definition != pop_types::FFI_ALLOCATION_ERROR_TYPE_ID
        }
        Some(
            SemanticType::Primitive(PrimitiveType::String)
            | SemanticType::Tuple(_)
            | SemanticType::Array(_)
            | SemanticType::Table { .. }
            | SemanticType::Class { .. }
            | SemanticType::Interface { .. }
            | SemanticType::Function { .. }
            | SemanticType::ErrorUnion { .. },
        ) => true,
        _ => false,
    }
}

pub(crate) fn task_group_object_map(
    cancel_type: TypeId,
    body_type: TypeId,
    completion_type: TypeId,
    arena: &TypeArena,
) -> ObjectMap {
    let references = [cancel_type, body_type, completion_type]
        .into_iter()
        .enumerate()
        .filter_map(|(index, type_id)| {
            if is_managed_reference_type_id(type_id, Some(arena)) {
                Some(ObjectSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
            } else {
                None
            }
        })
        .collect();
    ObjectMap::new(3, references)
        .expect("verified task-group captures form a canonical logical object map")
}

pub(crate) fn task_object_map(
    dispatch: &MirTaskDispatch,
    argument_types: &[TypeId],
    completion_type: TypeId,
    arena: &TypeArena,
) -> ObjectMap {
    let indirect_offset = u32::from(matches!(dispatch, MirTaskDispatch::Indirect(_)));
    let mut references = Vec::new();
    if indirect_offset != 0 {
        references.push(ObjectSlot::new(0));
    }
    references.extend(
        argument_types
            .iter()
            .enumerate()
            .filter_map(|(index, argument_type)| {
                if is_managed_reference_type_id(*argument_type, Some(arena)) {
                    Some(ObjectSlot::new(
                        u32::try_from(index)
                            .unwrap_or(u32::MAX)
                            .saturating_add(indirect_offset),
                    ))
                } else {
                    None
                }
            }),
    );
    let completion_slot = u32::try_from(argument_types.len())
        .unwrap_or(u32::MAX)
        .saturating_add(indirect_offset);
    if is_managed_reference_type_id(completion_type, Some(arena)) {
        references.push(ObjectSlot::new(completion_slot));
    }
    ObjectMap::new(completion_slot.saturating_add(1), references)
        .expect("verified task captures form a canonical logical object map")
}

pub(crate) fn local_instruction_effects(kind: &MirInstructionKind) -> MirEffectSummary {
    match kind {
        MirInstructionKind::CheckedIntegerAdd { .. }
        | MirInstructionKind::CheckedIntegerSubtract { .. }
        | MirInstructionKind::CheckedIntegerMultiply { .. }
        | MirInstructionKind::CheckedIntegerDivide { .. }
        | MirInstructionKind::CheckedIntegerRemainder { .. }
        | MirInstructionKind::IntegerNegate { .. }
        | MirInstructionKind::ConvertFloatToInteger { .. }
        | MirInstructionKind::ArrayGetChecked { .. }
        | MirInstructionKind::ListGetChecked { .. } => {
            MirEffectSummary::empty().with(MirEffect::MayTrap)
        }
        MirInstructionKind::ViewSlice { .. } => MirEffectSummary::empty().with(MirEffect::MayTrap),
        MirInstructionKind::ViewMaterialize { .. } => {
            MirEffectSummary::from_effects([MirEffect::Allocates, MirEffect::GcSafePoint])
        }
        MirInstructionKind::CodecEncode { .. } | MirInstructionKind::CodecDecode { .. } => {
            MirEffectSummary::from_effects([MirEffect::Allocates, MirEffect::GcSafePoint])
        }
        MirInstructionKind::ConvertInteger { source, target, .. }
            if NumericConversionKind::IntegerToInteger {
                source: *source,
                target: *target,
            }
            .may_trap() =>
        {
            MirEffectSummary::empty().with(MirEffect::MayTrap)
        }
        MirInstructionKind::ArraySet { element_map, .. }
        | MirInstructionKind::ArrayFill { element_map, .. }
        | MirInstructionKind::ListSet { element_map, .. } => {
            let effects = MirEffectSummary::empty().with(MirEffect::MayTrap);
            if *element_map == ArrayElementMap::ManagedReference {
                effects.with(MirEffect::WritesManagedReference)
            } else {
                effects
            }
        }
        MirInstructionKind::TableSet {
            key_map, value_map, ..
        } => {
            let effects = MirEffectSummary::from_effects([
                MirEffect::Allocates,
                MirEffect::MayUnwind,
                MirEffect::GcSafePoint,
            ]);
            if *key_map == ArrayElementMap::ManagedReference
                || *value_map == ArrayElementMap::ManagedReference
            {
                effects.with(MirEffect::WritesManagedReference)
            } else {
                effects
            }
        }
        MirInstructionKind::FunctionReference(_)
        | MirInstructionKind::TupleMake(_)
        | MirInstructionKind::ArrayMake { .. }
        | MirInstructionKind::TableMake { .. }
        | MirInstructionKind::ClassMake { .. }
        | MirInstructionKind::CaptureCellAllocate { .. }
        | MirInstructionKind::ClosureEnvironmentAllocate { .. } => {
            MirEffectSummary::from_effects([
                MirEffect::Allocates,
                MirEffect::MayUnwind,
                MirEffect::GcSafePoint,
            ])
        }
        MirInstructionKind::TaskCreate { .. } | MirInstructionKind::CancelSourceCreate => {
            MirEffectSummary::from_effects([
                MirEffect::Allocates,
                MirEffect::MayUnwind,
                MirEffect::GcSafePoint,
            ])
        }
        MirInstructionKind::TaskGroupCreate { .. } => MirEffectSummary::from_effects([
            MirEffect::Allocates,
            MirEffect::MayUnwind,
            MirEffect::GcSafePoint,
            MirEffect::Synchronizes,
        ]),
        MirInstructionKind::TaskStart { .. } => MirEffectSummary::from_effects([
            MirEffect::Synchronizes,
            MirEffect::MayUnwind,
            MirEffect::GcSafePoint,
        ]),
        MirInstructionKind::CancelRequest { .. } => {
            MirEffectSummary::empty().with(MirEffect::Synchronizes)
        }
        MirInstructionKind::CancelSourceToken { .. } => MirEffectSummary::empty(),
        MirInstructionKind::StringConcat { .. } | MirInstructionKind::StringFormat { .. } => {
            MirEffectSummary::from_effects([
                MirEffect::Allocates,
                MirEffect::MayUnwind,
                MirEffect::GcSafePoint,
            ])
        }
        MirInstructionKind::ArrayCreate { .. } => MirEffectSummary::from_effects([
            MirEffect::Allocates,
            MirEffect::MayTrap,
            MirEffect::MayUnwind,
            MirEffect::GcSafePoint,
        ]),
        MirInstructionKind::ListCreate { .. } | MirInstructionKind::RangeCreate { .. } => {
            MirEffectSummary::from_effects([
                MirEffect::Allocates,
                MirEffect::MayTrap,
                MirEffect::MayUnwind,
                MirEffect::GcSafePoint,
            ])
        }
        MirInstructionKind::ListAdd { element_map, .. } => {
            let effects = MirEffectSummary::from_effects([
                MirEffect::Allocates,
                MirEffect::MayUnwind,
                MirEffect::GcSafePoint,
            ]);
            if *element_map == ArrayElementMap::ManagedReference {
                effects.with(MirEffect::WritesManagedReference)
            } else {
                effects
            }
        }
        MirInstructionKind::GcSafePoint { .. } => {
            MirEffectSummary::empty().with(MirEffect::GcSafePoint)
        }
        MirInstructionKind::RetainRoot { .. }
        | MirInstructionKind::ReleaseRoot { .. }
        | MirInstructionKind::Pin { .. }
        | MirInstructionKind::Unpin { .. } => MirEffectSummary::empty().with(MirEffect::Roots),
        MirInstructionKind::FfiHandleOpen { .. }
        | MirInstructionKind::FfiHandleGet { .. }
        | MirInstructionKind::FfiHandleClose { .. } => {
            MirEffectSummary::from_effects([MirEffect::MayTrap, MirEffect::Roots])
        }
        MirInstructionKind::FfiBufferOpen { .. } => MirEffectSummary::from_effects([
            MirEffect::Allocates,
            MirEffect::MayTrap,
            MirEffect::GcSafePoint,
            MirEffect::Roots,
        ]),
        MirInstructionKind::FfiBufferClose { .. } => {
            MirEffectSummary::from_effects([MirEffect::MayTrap, MirEffect::Roots])
        }
        MirInstructionKind::FfiBufferLength { .. }
        | MirInstructionKind::FfiBufferRead { .. }
        | MirInstructionKind::FfiBufferWrite { .. }
        | MirInstructionKind::FfiBufferBorrow { .. }
        | MirInstructionKind::FfiBufferEndBorrow { .. } => {
            MirEffectSummary::empty().with(MirEffect::MayTrap)
        }
        MirInstructionKind::FfiBytesBorrow { .. }
        | MirInstructionKind::FfiBytesEndBorrow { .. } => {
            MirEffectSummary::from_effects([MirEffect::MayTrap, MirEffect::Roots])
        }
        MirInstructionKind::FfiBytesBorrowLength { .. } => {
            MirEffectSummary::empty().with(MirEffect::MayTrap)
        }
        MirInstructionKind::FfiCallbackOpenScoped { .. }
        | MirInstructionKind::FfiCallbackOpenOwned { .. } => MirEffectSummary::from_effects([
            MirEffect::Allocates,
            MirEffect::MayTrap,
            MirEffect::MayUnwind,
            MirEffect::GcSafePoint,
            MirEffect::Roots,
        ]),
        MirInstructionKind::CallCallbackPair {
            declared_effects, ..
        } => *declared_effects,
        MirInstructionKind::FfiCallbackCloseScoped { .. }
        | MirInstructionKind::FfiCallbackCloseOwned { .. } => {
            MirEffectSummary::from_effects([MirEffect::MayTrap, MirEffect::Roots])
        }
        MirInstructionKind::FfiUnsafeLoad { .. }
        | MirInstructionKind::FfiUnsafeStore { .. }
        | MirInstructionKind::FfiUnsafeAdvance { .. }
        | MirInstructionKind::FfiUnsafeCopy { .. } => {
            MirEffectSummary::from_effects([MirEffect::UnsafeMemory, MirEffect::MayTrap])
        }
        MirInstructionKind::FfiUnsafeAddress { .. }
        | MirInstructionKind::FfiUnsafePointerFromAddress { .. } => {
            MirEffectSummary::empty().with(MirEffect::UnsafeMemory)
        }
        MirInstructionKind::WriteBarrier { .. } => {
            MirEffectSummary::empty().with(MirEffect::WritesManagedReference)
        }
        MirInstructionKind::CaptureCellStore { .. } | MirInstructionKind::CaptureStore { .. } => {
            MirEffectSummary::from_effects([MirEffect::WritesManagedReference])
        }
        MirInstructionKind::CallDirect {
            declared_effects, ..
        }
        | MirInstructionKind::CallForeign {
            declared_effects, ..
        }
        | MirInstructionKind::CallReferenced {
            declared_effects, ..
        }
        | MirInstructionKind::CallStandard {
            declared_effects, ..
        }
        | MirInstructionKind::CallDirectMethod {
            declared_effects, ..
        }
        | MirInstructionKind::CallInterface {
            declared_effects, ..
        }
        | MirInstructionKind::CallBuiltinInterface {
            declared_effects, ..
        }
        | MirInstructionKind::CallIndirect {
            declared_effects, ..
        }
        | MirInstructionKind::CallScopedBorrow {
            declared_effects, ..
        } => *declared_effects,
        MirInstructionKind::IntegerConstant(_)
        | MirInstructionKind::FloatConstant(_)
        | MirInstructionKind::StringConstant(_)
        | MirInstructionKind::BooleanConstant(_)
        | MirInstructionKind::NilConstant
        | MirInstructionKind::FfiPointerNone
        | MirInstructionKind::FfiPointerToOptional { .. }
        | MirInstructionKind::FfiPointerReadOnly { .. }
        | MirInstructionKind::FfiPointerIsPresent { .. }
        | MirInstructionKind::FfiPointerRequire { .. }
        | MirInstructionKind::OptionalIsPresent { .. }
        | MirInstructionKind::OptionalGet { .. }
        | MirInstructionKind::ResultMake { .. }
        | MirInstructionKind::IterationMake { .. }
        | MirInstructionKind::ErrorMake { .. }
        | MirInstructionKind::ResultIsOk { .. }
        | MirInstructionKind::ResultGetOk { .. }
        | MirInstructionKind::ResultGetError { .. }
        | MirInstructionKind::EnumConstant { .. }
        | MirInstructionKind::TupleGet { .. }
        | MirInstructionKind::ArrayGet { .. }
        | MirInstructionKind::TableGet { .. }
        | MirInstructionKind::ArrayLength { .. }
        | MirInstructionKind::ListGet { .. }
        | MirInstructionKind::ListLength { .. }
        | MirInstructionKind::FloatAdd { .. }
        | MirInstructionKind::FloatSubtract { .. }
        | MirInstructionKind::FloatMultiply { .. }
        | MirInstructionKind::FloatDivide { .. }
        | MirInstructionKind::FloatNegate { .. }
        | MirInstructionKind::ConvertInteger { .. }
        | MirInstructionKind::ConvertIntegerToFloat { .. }
        | MirInstructionKind::ConvertFloat { .. }
        | MirInstructionKind::BooleanNot { .. }
        | MirInstructionKind::BooleanAnd { .. }
        | MirInstructionKind::BooleanOr { .. }
        | MirInstructionKind::CompareEqual { .. }
        | MirInstructionKind::CompareNotEqual { .. }
        | MirInstructionKind::CompareIntegerLess { .. }
        | MirInstructionKind::CompareIntegerLessOrEqual { .. }
        | MirInstructionKind::CompareIntegerGreater { .. }
        | MirInstructionKind::CompareIntegerGreaterOrEqual { .. }
        | MirInstructionKind::CompareFloatLess { .. }
        | MirInstructionKind::CompareFloatLessOrEqual { .. }
        | MirInstructionKind::CompareFloatGreater { .. }
        | MirInstructionKind::CompareFloatGreaterOrEqual { .. }
        | MirInstructionKind::RecordMake { .. }
        | MirInstructionKind::RecordUpdate { .. }
        | MirInstructionKind::CodecErrorConstant { .. }
        | MirInstructionKind::GeneratedCodecSchema(_)
        | MirInstructionKind::FieldGet { .. }
        | MirInstructionKind::FieldSet { .. }
        | MirInstructionKind::UnionMake { .. }
        | MirInstructionKind::IterationIsItem { .. }
        | MirInstructionKind::IterationGetItem { .. } => MirEffectSummary::empty(),
        MirInstructionKind::InterfaceUpcast { .. }
        | MirInstructionKind::CheckedDowncast { .. }
        | MirInstructionKind::ViewCreate { .. }
        | MirInstructionKind::ViewLength { .. }
        | MirInstructionKind::ViewGetByte { .. }
        | MirInstructionKind::ViewEnd { .. }
        | MirInstructionKind::CaptureCellLoad { .. }
        | MirInstructionKind::CaptureLoad { .. }
        | MirInstructionKind::CaptureCellReference { .. } => MirEffectSummary::empty(),
    }
}

pub(crate) fn terminator_effects(terminator: &MirTerminator) -> MirEffectSummary {
    match terminator {
        MirTerminator::Trap(_) => MirEffectSummary::empty().with(MirEffect::MayTrap),
        MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::ResumeUnwind => MirEffectSummary::empty().with(MirEffect::MayUnwind),
        MirTerminator::Suspend { .. } => MirEffectSummary::from_effects([
            MirEffect::Suspends,
            MirEffect::MayUnwind,
            MirEffect::GcSafePoint,
        ]),
        MirTerminator::Missing
        | MirTerminator::Branch { .. }
        | MirTerminator::ConditionalBranch { .. }
        | MirTerminator::UnionSwitch { .. }
        | MirTerminator::ErrorSwitch { .. }
        | MirTerminator::CodecErrorSwitch { .. }
        | MirTerminator::Return { .. }
        | MirTerminator::Unreachable => MirEffectSummary::empty(),
    }
}

fn recompute_effects(bubble: &mut MirBubble) {
    let foreign_effects = bubble
        .foreign_functions
        .iter()
        .map(|function| (function.symbol(), function.effects()))
        .collect();
    recompute_callable_effects(&mut bubble.functions, &mut bubble.methods, &foreign_effects);
}

fn recompute_callable_effects(
    functions: &mut [MirFunction],
    methods: &mut [MirMethod],
    foreign_effects: &BTreeMap<SymbolId, MirEffectSummary>,
) {
    let mut function_effects = foreign_effects.clone();
    function_effects.extend(
        functions
            .iter()
            .map(|function| (function.symbol, function.effects)),
    );
    let mut method_effects: BTreeMap<_, _> = methods
        .iter()
        .map(|method| (method.method, method.function.effects))
        .collect();
    loop {
        let mut changed = false;
        for function in &mut *functions {
            changed |= recompute_function_effects(function, &function_effects, &method_effects);
            function_effects.insert(function.symbol, function.effects);
        }
        for method in &mut *methods {
            changed |= recompute_function_effects(
                &mut method.function,
                &function_effects,
                &method_effects,
            );
            method_effects.insert(method.method, method.function.effects);
        }
        if !changed {
            break;
        }
    }
}

fn recompute_function_effects(
    function: &mut MirFunction,
    function_effects: &BTreeMap<SymbolId, MirEffectSummary>,
    method_effects: &BTreeMap<MethodId, MirEffectSummary>,
) -> bool {
    let mut summary = MirEffectSummary::empty();
    let mut changed = false;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let expected = match &instruction.kind {
                MirInstructionKind::CallDirect { function, .. } => {
                    function_effects.get(function).copied().unwrap_or_default()
                }
                MirInstructionKind::CallForeign { function, .. } => {
                    function_effects.get(function).copied().unwrap_or_default()
                }
                MirInstructionKind::CallDirectMethod { method, .. } => {
                    method_effects.get(method).copied().unwrap_or_default()
                }
                MirInstructionKind::CallIndirect {
                    declared_effects, ..
                } => *declared_effects,
                _ => local_instruction_effects(&instruction.kind),
            };
            if !instruction.effects_explicit && instruction.effects != expected {
                instruction.effects = expected;
                changed = true;
            }
            if !instruction.effects_explicit {
                match &mut instruction.kind {
                    MirInstructionKind::CallDirect {
                        declared_effects, ..
                    }
                    | MirInstructionKind::CallForeign {
                        declared_effects, ..
                    }
                    | MirInstructionKind::CallDirectMethod {
                        declared_effects, ..
                    } => *declared_effects = expected,
                    _ => {}
                }
            }
            summary = summary.union(expected);
        }
        summary = summary.union(terminator_effects(&block.terminator));
    }
    if !function.effects_explicit && function.effects != summary {
        function.effects = summary;
        changed = true;
    }
    changed
}

pub(crate) fn insert_gc_safe_points(bubble: &mut MirBubble, arena: &TypeArena) -> bool {
    let mut changed = false;
    for function in &mut bubble.functions {
        changed |= insert_function_safe_points(function, arena);
    }
    for method in &mut bubble.methods {
        changed |= insert_function_safe_points(&mut method.function, arena);
    }
    for nested in &mut bubble.nested_functions {
        let mut function = nested.transformation_adapter();
        changed |= insert_function_safe_points(&mut function, arena);
        nested.apply_transformation(function);
    }
    changed
}

fn insert_function_safe_points(function: &mut MirFunction, arena: &TypeArena) -> bool {
    let mut next_value = function
        .blocks
        .iter()
        .flat_map(|block| {
            block.arguments.iter().map(|argument| argument.value).chain(
                block
                    .instructions
                    .iter()
                    .map(|instruction| instruction.result),
            )
        })
        .map(ValueId::raw)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut next_safe_point = function
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .instructions
                .iter()
                .filter_map(|instruction| match instruction.kind {
                    MirInstructionKind::GcSafePoint { safe_point, .. } => Some(safe_point.raw()),
                    _ => None,
                })
                .chain(match block.terminator {
                    MirTerminator::Suspend { safe_point, .. } => Some(safe_point.raw()),
                    _ => None,
                })
        })
        .max()
        .map_or(0, |safe_point| safe_point.saturating_add(1));
    let mut changed = false;
    for block in &mut function.blocks {
        let mut instructions: Vec<MirInstruction> = Vec::new();
        let mut straight_line_work = 0_usize;
        for instruction in std::mem::take(&mut block.instructions) {
            let is_safe_point = matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. });
            let operation_requires_safe_point = instruction.effects.contains(MirEffect::Allocates)
                || matches!(
                    instruction.kind,
                    MirInstructionKind::CallDirect { .. }
                        | MirInstructionKind::CallForeign { .. }
                        | MirInstructionKind::CallDirectMethod { .. }
                        | MirInstructionKind::CallInterface { .. }
                        | MirInstructionKind::CallIndirect { .. }
                        | MirInstructionKind::CallScopedBorrow { .. }
                ) && instruction.effects.contains(MirEffect::GcSafePoint);
            let already_at_safe_point = instructions.last().is_some_and(|previous| {
                matches!(previous.kind, MirInstructionKind::GcSafePoint { .. })
            });
            if !is_safe_point
                && !already_at_safe_point
                && (operation_requires_safe_point
                    || straight_line_work >= MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS)
            {
                instructions.push(empty_safe_point(
                    ValueId::from_raw(next_value),
                    SafePointId::new(next_safe_point),
                    instruction.span,
                ));
                next_value = next_value.saturating_add(1);
                next_safe_point = next_safe_point.saturating_add(1);
                straight_line_work = 0;
                changed = true;
            }
            instructions.push(instruction);
            if is_safe_point {
                straight_line_work = 0;
            } else {
                straight_line_work = straight_line_work.saturating_add(1);
            }
        }
        let has_backedge = terminator_targets(&block.terminator)
            .into_iter()
            .any(|target| target <= block.block);
        if has_backedge
            && !instructions.last().is_some_and(|instruction| {
                matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. })
            })
        {
            instructions.push(empty_safe_point(
                ValueId::from_raw(next_value),
                SafePointId::new(next_safe_point),
                SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0))),
            ));
            next_value = next_value.saturating_add(1);
            next_safe_point = next_safe_point.saturating_add(1);
            changed = true;
        }
        block.instructions = instructions;
    }
    populate_stack_maps(function, arena);
    populate_suspend_frames(function, arena);
    changed
}

fn empty_safe_point(result: ValueId, safe_point: SafePointId, span: SourceSpan) -> MirInstruction {
    MirInstruction {
        result,
        result_type: None,
        kind: MirInstructionKind::GcSafePoint {
            safe_point,
            roots: Vec::new(),
            stack_map: StackMap::new(safe_point, Vec::new()).expect("empty stack map is canonical"),
        },
        effects: MirEffectSummary::empty().with(MirEffect::GcSafePoint),
        effects_explicit: true,
        unwind: MirUnwindAction::Propagate,
        span,
    }
}

fn populate_stack_maps(function: &mut MirFunction, arena: &TypeArena) {
    let expected = expected_safe_point_roots(function, arena);
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let MirInstructionKind::GcSafePoint {
                safe_point,
                roots,
                stack_map,
            } = &mut instruction.kind
            else {
                continue;
            };
            *roots = expected
                .get(&instruction.result)
                .cloned()
                .unwrap_or_default();
            let root_slots = (0..roots.len())
                .map(|index| RootSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
                .collect();
            *stack_map = StackMap::new(*safe_point, root_slots)
                .expect("generated stack root slots are unique");
        }
    }
}

fn populate_suspend_frames(function: &mut MirFunction, arena: &TypeArena) {
    let expected = expected_suspend_frame_slots(function);
    for block in &mut function.blocks {
        let MirTerminator::Suspend {
            operation: MirSuspendOperation::Task { task, .. },
            safe_point,
            live_frame,
            ..
        } = &mut block.terminator
        else {
            continue;
        };
        let _ = task;
        live_frame.slots = expected.get(&block.block).cloned().unwrap_or_default();
        let roots = live_frame
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                if is_managed_reference_type_id(slot.type_id, Some(arena)) {
                    Some(RootSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
                } else {
                    None
                }
            })
            .collect();
        live_frame.stack_map = StackMap::new(*safe_point, roots)
            .expect("generated coroutine frame root slots are unique");
    }
}

pub(crate) fn expected_suspend_frame_slots(
    function: &MirFunction,
) -> BTreeMap<BlockId, Vec<MirFrameSlot>> {
    let (value_types, _, live_out) = live_value_facts(function);
    function
        .blocks
        .iter()
        .filter_map(|block| {
            let MirTerminator::Suspend {
                operation: MirSuspendOperation::Task { task, .. },
                ..
            } = block.terminator()
            else {
                return None;
            };
            let mut live = live_out.get(&block.block).cloned().unwrap_or_default();
            live.insert(*task);
            let slots = live
                .into_iter()
                .filter_map(|value| {
                    value_types
                        .get(&value)
                        .copied()
                        .map(|type_id| MirFrameSlot { value, type_id })
                })
                .collect();
            Some((block.block(), slots))
        })
        .collect()
}

type LiveValueFacts = (
    BTreeMap<ValueId, TypeId>,
    BTreeMap<BlockId, BTreeSet<ValueId>>,
    BTreeMap<BlockId, BTreeSet<ValueId>>,
);

fn live_value_facts(function: &MirFunction) -> LiveValueFacts {
    let blocks: BTreeMap<_, _> = function
        .blocks
        .iter()
        .map(|block| (block.block(), block))
        .collect();
    let mut value_types = BTreeMap::new();
    for block in &function.blocks {
        for argument in &block.arguments {
            value_types.insert(argument.value, argument.type_id);
        }
        for instruction in &block.instructions {
            if let Some(type_id) = instruction.result_type {
                value_types.insert(instruction.result, type_id);
            }
        }
    }
    let mut live_in: BTreeMap<BlockId, BTreeSet<ValueId>> = function
        .blocks
        .iter()
        .map(|block| (block.block, BTreeSet::new()))
        .collect();
    let mut live_out = live_in.clone();
    loop {
        let mut changed = false;
        for block in function.blocks.iter().rev() {
            let outgoing = normal_live_out(block, &live_in, &blocks);
            let mut live = outgoing.clone();
            live.extend(terminator_operands(&block.terminator));
            for instruction in block.instructions.iter().rev() {
                if instruction.has_result() {
                    live.remove(&instruction.result);
                }
                if !matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. }) {
                    if let Some(target) = instruction_unwind_target(instruction)
                        && let Some(cleanup_live) = live_in.get(&target)
                    {
                        live.extend(cleanup_live.iter().copied());
                    }
                    live.extend(instruction_operands(&instruction.kind));
                }
            }
            if live_out.get(&block.block) != Some(&outgoing) {
                live_out.insert(block.block, outgoing);
                changed = true;
            }
            if live_in.get(&block.block) != Some(&live) {
                live_in.insert(block.block, live);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (value_types, live_in, live_out)
}

pub(crate) fn expected_safe_point_roots(
    function: &MirFunction,
    arena: &TypeArena,
) -> BTreeMap<ValueId, Vec<ValueId>> {
    let (value_types, live_in, live_out) = live_value_facts(function);
    let view_lenders = view_lender_roots(function, arena, &value_types);

    let mut maps = BTreeMap::new();
    for block in &function.blocks {
        let mut live = live_out.get(&block.block).cloned().unwrap_or_default();
        live.extend(terminator_operands(&block.terminator));
        for instruction in block.instructions.iter().rev() {
            if let MirInstructionKind::GcSafePoint { .. } = instruction.kind {
                let roots = live
                    .iter()
                    .filter_map(|value| {
                        value_types.get(value).and_then(|type_id| {
                            if is_managed_reference_type_id(*type_id, Some(arena)) {
                                Some(*value)
                            } else {
                                view_lenders.get(value).copied()
                            }
                        })
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                maps.insert(instruction.result, roots);
            }
            if instruction.has_result() {
                live.remove(&instruction.result);
            }
            if !matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. }) {
                if let Some(target) = instruction_unwind_target(instruction)
                    && let Some(cleanup_live) = live_in.get(&target)
                {
                    live.extend(cleanup_live.iter().copied());
                }
                live.extend(instruction_operands(&instruction.kind));
            }
        }
    }
    maps
}

fn view_lender_roots(
    function: &MirFunction,
    arena: &TypeArena,
    value_types: &BTreeMap<ValueId, TypeId>,
) -> BTreeMap<ValueId, ValueId> {
    let mut lenders = BTreeMap::new();
    if let Some(entry) = function.blocks().first() {
        for argument in entry.arguments() {
            if value_types
                .get(&argument.value())
                .is_some_and(|type_id| view_kind_for_gc(arena, *type_id))
            {
                lenders.insert(argument.value(), argument.value());
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            for instruction in block.instructions() {
                let lender = match instruction.kind() {
                    MirInstructionKind::ViewCreate { lender, .. } => Some(*lender),
                    MirInstructionKind::ViewSlice { view, .. } => lenders.get(view).copied(),
                    MirInstructionKind::CallDirect {
                        arguments,
                        view_result: Some(result),
                        ..
                    }
                    | MirInstructionKind::CallReferenced {
                        arguments,
                        view_result: Some(result),
                        ..
                    } => arguments
                        .get(usize::from(result.source_argument()))
                        .map(|source| lenders.get(source).copied().unwrap_or(*source)),
                    _ => None,
                };
                if let Some(lender) = lender
                    && lenders.insert(instruction.result(), lender) != Some(lender)
                {
                    changed = true;
                }
            }
            if let MirTerminator::Branch { target, arguments } = block.terminator()
                && let Some(target) = function
                    .blocks()
                    .iter()
                    .find(|block| block.block() == *target)
            {
                for (source, target) in arguments.iter().zip(target.arguments()) {
                    if let Some(lender) = lenders.get(source).copied()
                        && lenders.insert(target.value(), lender) != Some(lender)
                    {
                        changed = true;
                    }
                }
            }
        }
    }
    lenders
}

fn view_kind_for_gc(arena: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        arena.get(type_id),
        Some(SemanticType::Builtin { definition, arguments })
            if arguments.is_empty()
                && matches!(
                    *definition,
                    pop_types::BYTES_VIEW_TYPE_ID | pop_types::TEXT_VIEW_TYPE_ID
                )
    )
}

fn normal_live_out(
    block: &MirBlock,
    live_in: &BTreeMap<BlockId, BTreeSet<ValueId>>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeSet<ValueId> {
    match block.terminator() {
        MirTerminator::Branch { target, arguments } => {
            edge_live_values(*target, arguments, live_in, blocks)
        }
        MirTerminator::ConditionalBranch {
            when_true,
            when_false,
            ..
        } => {
            let mut outgoing = edge_live_values(*when_true, &[], live_in, blocks);
            outgoing.extend(edge_live_values(*when_false, &[], live_in, blocks));
            outgoing
        }
        MirTerminator::UnionSwitch { arms, .. } => arms
            .iter()
            .flat_map(|arm| edge_live_values(arm.target, &[], live_in, blocks))
            .collect(),
        MirTerminator::ErrorSwitch { arms, .. } => arms
            .iter()
            .flat_map(|arm| edge_live_values(arm.target, &[], live_in, blocks))
            .collect(),
        MirTerminator::CodecErrorSwitch { arms, .. } => arms
            .iter()
            .flat_map(|arm| edge_live_values(arm.target, &[], live_in, blocks))
            .collect(),
        MirTerminator::Suspend {
            resume,
            cancellation,
            unwind,
            ..
        } => {
            let mut outgoing = edge_live_values(*resume, &[], live_in, blocks);
            outgoing.extend(edge_live_values(*cancellation, &[], live_in, blocks));
            if let MirUnwindAction::Cleanup(target) = unwind {
                outgoing.extend(edge_live_values(*target, &[], live_in, blocks));
            }
            outgoing
        }
        MirTerminator::Missing
        | MirTerminator::Return { .. }
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::ResumeUnwind
        | MirTerminator::Unreachable => BTreeSet::new(),
    }
}

fn edge_live_values(
    target: BlockId,
    arguments: &[ValueId],
    live_in: &BTreeMap<BlockId, BTreeSet<ValueId>>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeSet<ValueId> {
    let mut live = live_in.get(&target).cloned().unwrap_or_default();
    let Some(target) = blocks.get(&target) else {
        return live;
    };
    for (index, parameter) in target.arguments().iter().enumerate() {
        if live.remove(&parameter.value())
            && let Some(argument) = arguments.get(index)
        {
            live.insert(*argument);
        }
    }
    live
}
