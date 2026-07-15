//! Independent verification for canonical backend-neutral MIR.
//!
//! Every construction and transforming pass uses this verifier. It owns CFG,
//! type, call, effect, failure, root, barrier, and safe-point invariants; a
//! backend receives MIR only after these checks succeed.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    BlockId, ClassId, ErrorId, FieldId, InterfaceId, MethodId, NominalInterfaceId, SymbolId,
    SymbolIdentity, TypeId, UnionCaseId, ValueId,
};
use pop_runtime_interface::{ArrayElementMap, ObjectMap, ObjectSlot, RootSlot};
use pop_types::{FloatKind, IntegerKind, SemanticType, TypeArena, embedded_bootstrap_schema};

use crate::ir::*;
use crate::lowering::{
    array_element_map, expected_safe_point_roots, expected_suspend_frame_slots,
    is_managed_reference_type_id, list_element_map, local_instruction_effects, table_element_maps,
    task_object_map, terminator_effects,
};
use crate::render::{float_kind_text, integer_kind_text};

/// Verifies canonical MIR block, value, type, call, and return invariants.
///
/// # Errors
///
/// Returns deterministic invariant violations.
pub fn verify_mir_bubble(
    bubble: &MirBubble,
    arena: &TypeArena,
) -> Result<(), Vec<MirVerificationError>> {
    let signatures: BTreeMap<_, _> = bubble
        .functions
        .iter()
        .map(|function| {
            (
                function.symbol(),
                (
                    function.parameters().to_vec(),
                    function.results().to_vec(),
                    function.effects(),
                ),
            )
        })
        .collect();
    let method_signatures: BTreeMap<_, _> = bubble
        .methods
        .iter()
        .map(|method| {
            (
                method.method,
                (
                    method.function.parameters().to_vec(),
                    method.function.results().to_vec(),
                    method.function.effects(),
                ),
            )
        })
        .collect();
    let async_functions: BTreeSet<_> = bubble
        .functions
        .iter()
        .filter(|function| function.is_async())
        .map(MirFunction::symbol)
        .collect();
    let mut errors = Vec::new();
    let mut reference_signatures = BTreeMap::new();
    for reference in &bubble.function_references {
        let signature = (
            reference.parameters.clone(),
            reference.results.clone(),
            reference.effects,
        );
        if reference_signatures
            .insert(reference.identity, signature)
            .is_some()
        {
            errors.push(MirVerificationError::DuplicateReferencedFunction(
                reference.identity,
            ));
        }
        if !bubble.dependencies.contains(&reference.identity.bubble()) {
            errors.push(MirVerificationError::UnknownReferencedFunction(
                reference.identity,
            ));
        }
    }
    let async_references: BTreeSet<_> = bubble
        .function_references
        .iter()
        .filter(|reference| reference.is_async())
        .map(MirFunctionReference::identity)
        .collect();
    let schema = MirSchema::collect(bubble, arena, &method_signatures, &mut errors);
    for function in &bubble.functions {
        verify_function(
            function,
            arena,
            &schema,
            &signatures,
            &reference_signatures,
            &method_signatures,
            &async_functions,
            &async_references,
            &mut errors,
        );
    }
    for method in &bubble.methods {
        verify_function(
            &method.function,
            arena,
            &schema,
            &signatures,
            &reference_signatures,
            &method_signatures,
            &async_functions,
            &async_references,
            &mut errors,
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[derive(Clone)]
struct DeclaredField {
    owner_types: BTreeSet<TypeId>,
    field_type: TypeId,
    mutable: bool,
}

struct MirSchema<'mir> {
    records: BTreeMap<SymbolId, &'mir MirRecordDeclaration>,
    unions: BTreeMap<SymbolId, &'mir MirUnionDeclaration>,
    errors: BTreeMap<ErrorId, &'mir MirErrorDeclaration>,
    enums: BTreeMap<SymbolId, &'mir MirEnumDeclaration>,
    classes: BTreeMap<ClassId, &'mir MirClassDeclaration>,
    interfaces: BTreeMap<InterfaceId, &'mir MirInterfaceDeclaration>,
    fields: BTreeMap<FieldId, DeclaredField>,
}

impl<'mir> MirSchema<'mir> {
    fn collect(
        bubble: &'mir MirBubble,
        arena: &TypeArena,
        method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
        errors: &mut Vec<MirVerificationError>,
    ) -> Self {
        let mut schema = Self {
            records: BTreeMap::new(),
            unions: BTreeMap::new(),
            errors: BTreeMap::new(),
            enums: BTreeMap::new(),
            classes: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            fields: BTreeMap::new(),
        };
        let mut symbols = BTreeSet::new();
        for declaration in &bubble.declarations {
            if !symbols.insert(declaration.symbol) {
                errors.push(MirVerificationError::DuplicateDeclaration(
                    declaration.symbol,
                ));
            }
            match &declaration.kind {
                MirDeclarationKind::Record(record) => {
                    if !matches!(arena.get(record.type_id), Some(SemanticType::Record(_))) {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: record.type_id,
                        });
                    }
                    schema.records.insert(declaration.symbol, record);
                    schema.collect_fields(record.type_id, &record.fields, false, errors);
                }
                MirDeclarationKind::Union(union) => {
                    if !matches!(
                        arena.get(union.type_id),
                        Some(SemanticType::TaggedUnion { .. })
                    ) {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: union.type_id,
                        });
                    }
                    let mut cases = BTreeSet::new();
                    for case in &union.cases {
                        if !cases.insert(case.case) {
                            errors.push(MirVerificationError::DuplicateUnionCase {
                                union: declaration.symbol,
                                case: case.case,
                            });
                        }
                    }
                    schema.unions.insert(declaration.symbol, union);
                }
                MirDeclarationKind::Error(error) => {
                    if !matches!(
                        arena.get(error.type_id),
                        Some(SemanticType::ErrorUnion { definition, .. }) if *definition == error.error
                    ) {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: error.type_id,
                        });
                    }
                    let mut cases = BTreeSet::new();
                    for case in &error.cases {
                        if !cases.insert(case.case) {
                            errors.push(MirVerificationError::DuplicateErrorCase {
                                error: error.error,
                                case: case.case,
                            });
                        }
                    }
                    schema.errors.insert(error.error, error);
                }
                MirDeclarationKind::Enum(enumeration) => {
                    if arena.get(enumeration.type_id)
                        != Some(&SemanticType::Enum {
                            definition: declaration.symbol,
                        })
                    {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: enumeration.type_id,
                        });
                    }
                    schema.enums.insert(declaration.symbol, enumeration);
                }
                MirDeclarationKind::Class(class) => {
                    if !matches!(
                        arena.get(class.type_id),
                        Some(SemanticType::Class { class: identity, arguments })
                            if *identity == class.class
                                && arguments
                                    .iter()
                                    .all(|argument| !arena.contains_type_parameter(*argument))
                    ) {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: class.type_id,
                        });
                    }
                    if schema.classes.insert(class.class, class).is_some() {
                        errors.push(MirVerificationError::DuplicateClass(class.class));
                    }
                    verify_builtin_interface_implementations(
                        class,
                        arena,
                        method_signatures,
                        errors,
                    );
                    schema.collect_fields(class.type_id, &class.fields, true, errors);
                }
                MirDeclarationKind::Interface(interface) => {
                    if !matches!(
                        arena.get(interface.type_id),
                        Some(SemanticType::Interface { interface: identity, arguments })
                            if *identity == interface.interface
                                && arguments
                                    .iter()
                                    .all(|argument| !arena.contains_type_parameter(*argument))
                    ) {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: interface.type_id,
                        });
                    }
                    schema.interfaces.insert(interface.interface, interface);
                }
            }
        }
        schema.verify_interface_implementations(method_signatures, errors);
        schema
    }

    fn verify_interface_implementations(
        &self,
        method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
        errors: &mut Vec<MirVerificationError>,
    ) {
        for class in self.classes.values() {
            let mut interfaces = BTreeSet::new();
            for implementation in class.interfaces() {
                let Some(interface) = self.interfaces.get(&implementation.interface()) else {
                    errors.push(MirVerificationError::InvalidInterfaceImplementation {
                        class: class.class(),
                        interface: implementation.interface(),
                    });
                    continue;
                };
                let mut methods = BTreeSet::new();
                let valid = interfaces.insert(implementation.interface())
                    && implementation.interface_type() == interface.type_id()
                    && implementation.methods().len() == interface.methods().len()
                    && implementation.methods().iter().all(|mapping| {
                        let Some(required) = interface
                            .methods()
                            .iter()
                            .find(|method| method.method() == mapping.interface_method())
                        else {
                            return false;
                        };
                        methods.insert(mapping.interface_method())
                            && mapping.slot() == required.slot()
                            && class.methods().contains(&mapping.class_method())
                            && method_signatures.get(&mapping.class_method()).is_some_and(
                                |(parameters, results, _)| {
                                    parameters.first() == Some(&class.type_id())
                                        && parameters[1..] == required.parameters()[..]
                                        && results == required.results()
                                },
                            )
                    })
                    && interface
                        .methods()
                        .iter()
                        .all(|required| methods.contains(&required.method()));
                if !valid {
                    errors.push(MirVerificationError::InvalidInterfaceImplementation {
                        class: class.class(),
                        interface: implementation.interface(),
                    });
                }
            }
        }
    }

    fn collect_fields(
        &mut self,
        owner_type: TypeId,
        fields: &[MirField],
        mutable: bool,
        errors: &mut Vec<MirVerificationError>,
    ) {
        for field in fields {
            if let Some(existing) = self.fields.get_mut(&field.field) {
                if existing.field_type != field.field_type || existing.mutable != mutable {
                    errors.push(MirVerificationError::DuplicateDeclaredField(field.field));
                } else {
                    existing.owner_types.insert(owner_type);
                }
                continue;
            }
            self.fields.insert(
                field.field,
                DeclaredField {
                    owner_types: BTreeSet::from([owner_type]),
                    field_type: field.field_type,
                    mutable,
                },
            );
        }
    }
}

fn verify_builtin_interface_implementations(
    class: &MirClassDeclaration,
    arena: &TypeArena,
    method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(protocol) = embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol())
    else {
        return;
    };
    let mut interfaces = BTreeSet::new();
    for implementation in class.builtin_interfaces() {
        let item_type = match arena.get(implementation.interface_type()) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if *definition == implementation.interface() && arguments.len() == 1 => arguments[0],
            _ => {
                errors.push(
                    MirVerificationError::InvalidBuiltinInterfaceImplementation {
                        class: class.class(),
                        interface: implementation.interface(),
                    },
                );
                continue;
            }
        };
        let expected_protocol_methods = if implementation.interface() == protocol.iterable() {
            vec![protocol.iterator_method()]
        } else if implementation.interface() == protocol.iterator() {
            vec![protocol.iterator_method(), protocol.next_method()]
        } else {
            Vec::new()
        };
        let iterator_type = arena.find(&SemanticType::Builtin {
            definition: protocol.iterator(),
            arguments: vec![item_type],
        });
        let iteration_type = arena.find(&SemanticType::Builtin {
            definition: protocol.iteration(),
            arguments: vec![item_type],
        });
        let mut protocol_methods = BTreeSet::new();
        let valid = interfaces.insert(implementation.interface())
            && !expected_protocol_methods.is_empty()
            && implementation.methods().len() == expected_protocol_methods.len()
            && implementation.methods().iter().all(|mapping| {
                let expected_result = if mapping.protocol_method() == protocol.iterator_method() {
                    iterator_type
                } else if mapping.protocol_method() == protocol.next_method()
                    && implementation.interface() == protocol.iterator()
                {
                    iteration_type
                } else {
                    None
                };
                protocol_methods.insert(mapping.protocol_method())
                    && expected_protocol_methods.contains(&mapping.protocol_method())
                    && class.methods().contains(&mapping.class_method())
                    && expected_result.is_some_and(|expected_result| {
                        method_signatures.get(&mapping.class_method()).is_some_and(
                            |(parameters, results, _)| {
                                parameters.as_slice() == [class.type_id()]
                                    && results.as_slice() == [expected_result]
                            },
                        )
                    })
            })
            && expected_protocol_methods
                .iter()
                .all(|method| protocol_methods.contains(method));
        if !valid {
            errors.push(
                MirVerificationError::InvalidBuiltinInterfaceImplementation {
                    class: class.class(),
                    interface: implementation.interface(),
                },
            );
        }
    }
}

fn verify_function(
    function: &MirFunction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    signatures: &BTreeMap<SymbolId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    reference_signatures: &BTreeMap<SymbolIdentity, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    async_functions: &BTreeSet<SymbolId>,
    async_references: &BTreeSet<SymbolIdentity>,
    errors: &mut Vec<MirVerificationError>,
) {
    verify_entry_parameters(function, errors);
    let blocks = collect_blocks(function, errors);
    let cleanup_targets: BTreeSet<_> = function
        .blocks()
        .iter()
        .flat_map(|block| {
            block
                .instructions()
                .iter()
                .filter_map(instruction_unwind_target)
                .chain(match block.terminator() {
                    MirTerminator::Suspend {
                        unwind: MirUnwindAction::Cleanup(target),
                        ..
                    } => Some(*target),
                    _ => None,
                })
        })
        .collect();
    let mut unwind_cleanup_reachable = cleanup_targets.clone();
    let mut pending_cleanup_blocks: Vec<_> = cleanup_targets.iter().copied().collect();
    while let Some(block) = pending_cleanup_blocks.pop() {
        let Some(block) = blocks.get(&block) else {
            continue;
        };
        for target in terminator_targets(block.terminator()) {
            if unwind_cleanup_reachable.insert(target) {
                pending_cleanup_blocks.push(target);
            }
        }
    }
    let mut definitions = DefinitionTables::default();
    for block in &function.blocks {
        for argument in &block.arguments {
            definitions.collect(
                argument.value,
                argument.type_id,
                DefinitionSite {
                    block: block.block,
                    instruction: None,
                },
                arena,
                errors,
            );
        }
        for (index, instruction) in block.instructions.iter().enumerate() {
            if let Some(result_type) = instruction.result_type {
                definitions.collect(
                    instruction.result,
                    result_type,
                    DefinitionSite {
                        block: block.block,
                        instruction: Some(index),
                    },
                    arena,
                    errors,
                );
            } else if matches!(instruction.kind, MirInstructionKind::RetainRoot { .. }) {
                definitions.collect_root_handle(
                    instruction.result,
                    DefinitionSite {
                        block: block.block,
                        instruction: Some(index),
                    },
                    errors,
                );
            } else if matches!(instruction.kind, MirInstructionKind::Pin { .. }) {
                definitions.collect_pin_handle(
                    instruction.result,
                    DefinitionSite {
                        block: block.block,
                        instruction: Some(index),
                    },
                    errors,
                );
            } else if !definitions.seen.insert(instruction.result) {
                errors.push(MirVerificationError::DuplicateValue(instruction.result));
            }
        }
    }
    let dominators = compute_dominators(function, &blocks);
    let optional_presence = compute_optional_presence_facts(function, &blocks);
    let facts = ControlFlowFacts {
        values: &definitions.values,
        root_handles: &definitions.root_handles,
        pin_handles: &definitions.pin_handles,
        definitions: &definitions.sites,
        dominators: &dominators,
        blocks: &blocks,
    };
    let expected_suspend_frames = expected_suspend_frame_slots(function);
    let mut safe_points = BTreeSet::new();
    let mut coroutine_states = BTreeSet::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let MirInstructionKind::GcSafePoint { safe_point, .. } = instruction.kind()
                && !safe_points.insert(*safe_point)
            {
                errors.push(MirVerificationError::DuplicateSafePoint(*safe_point));
            }
        }
        if let MirTerminator::Suspend {
            safe_point,
            live_frame,
            ..
        } = block.terminator()
        {
            if !safe_points.insert(*safe_point) {
                errors.push(MirVerificationError::DuplicateSafePoint(*safe_point));
            }
            if !coroutine_states.insert(live_frame.state) {
                errors.push(MirVerificationError::DuplicateCoroutineState(
                    live_frame.state,
                ));
            }
        }
    }
    let mut required_function_effects = MirEffectSummary::empty();
    for block in &function.blocks {
        if let Some(cleanup) = block.cleanup() {
            if block.block() == BlockId::from_raw(0)
                || matches!(block.terminator(), MirTerminator::ResumeUnwind)
                    && cleanup.reason() != MirCleanupExitReason::Unwind
            {
                errors.push(MirVerificationError::InvalidCleanupBlock {
                    block: block.block(),
                });
            }
            for target in terminator_targets(block.terminator()) {
                if let Some(target_cleanup) = blocks.get(&target).and_then(|block| block.cleanup())
                    && (target_cleanup.reason() != cleanup.reason()
                        || target_cleanup.scope() > cleanup.scope())
                {
                    errors.push(MirVerificationError::InvalidCleanupBlock {
                        block: block.block(),
                    });
                }
            }
        }
        for (index, instruction) in block.instructions.iter().enumerate() {
            for operand in instruction_operands(&instruction.kind) {
                verify_value_use(operand, block.block, index, &facts, errors);
            }
            if let MirInstructionKind::OptionalGet { optional } = instruction.kind()
                && !optional_presence
                    .get(&block.block())
                    .is_some_and(|present| present.contains(optional))
            {
                errors.push(MirVerificationError::OptionalGetWithoutPresence {
                    instruction: instruction.result(),
                    optional: *optional,
                });
            }
            let referenced_function = match instruction.kind() {
                MirInstructionKind::CallDirect { function, .. }
                | MirInstructionKind::FunctionReference(function) => Some(*function),
                _ => None,
            };
            if let Some(function) = referenced_function
                && !signatures.contains_key(&function)
            {
                errors.push(MirVerificationError::UnknownFunction(function));
            }
            if let MirInstructionKind::CallReferenced { function, .. } = instruction.kind()
                && !reference_signatures.contains_key(function)
            {
                errors.push(MirVerificationError::UnknownReferencedFunction(*function));
            }
            if let MirInstructionKind::CallDirectMethod { method, .. } = instruction.kind()
                && !method_signatures.contains_key(method)
            {
                errors.push(MirVerificationError::UnknownMethod(*method));
            }
            verify_instruction_types(
                instruction,
                arena,
                schema,
                facts.values,
                CallableSignatures {
                    functions: signatures,
                    references: reference_signatures,
                    methods: method_signatures,
                    async_functions,
                    async_references,
                },
                errors,
            );
            let expected_effects = expected_instruction_effects(
                instruction,
                signatures,
                reference_signatures,
                method_signatures,
            );
            required_function_effects = required_function_effects.union(expected_effects);
            if instruction.effects() != expected_effects {
                errors.push(MirVerificationError::InstructionEffectMismatch {
                    instruction: instruction.result(),
                    expected: expected_effects,
                    found: instruction.effects(),
                });
            }
            verify_unwind_action(instruction, &blocks, errors);
        }
        required_function_effects =
            required_function_effects.union(terminator_effects(block.terminator()));
        verify_terminator(
            block,
            function,
            arena,
            schema,
            &facts,
            &expected_suspend_frames,
            errors,
        );
        if matches!(block.terminator(), MirTerminator::ResumeUnwind)
            && (!unwind_cleanup_reachable.contains(&block.block())
                || !block
                    .cleanup()
                    .is_some_and(|cleanup| cleanup.reason() == MirCleanupExitReason::Unwind))
        {
            errors.push(MirVerificationError::ResumeOutsideCleanup {
                block: block.block(),
            });
        }
    }
    if !required_function_effects.is_subset_of(function.effects()) {
        errors.push(MirVerificationError::FunctionEffectMismatch {
            function: function.symbol(),
            expected: required_function_effects,
            found: function.effects(),
        });
    }
    verify_gc_contracts(function, arena, schema, &facts, errors);
}

fn verify_entry_parameters(function: &MirFunction, errors: &mut Vec<MirVerificationError>) {
    let Some(entry) = function.blocks.first() else {
        return;
    };
    if entry.arguments.len() != function.parameters.len() {
        errors.push(MirVerificationError::EntryParameterArity {
            expected: function.parameters.len(),
            found: entry.arguments.len(),
        });
    }
    for (index, (argument, expected)) in
        entry.arguments.iter().zip(&function.parameters).enumerate()
    {
        if argument.type_id != *expected {
            errors.push(MirVerificationError::EntryParameterType {
                index,
                expected: *expected,
                found: argument.type_id,
            });
        }
    }
}

fn expected_instruction_effects(
    instruction: &MirInstruction,
    signatures: &BTreeMap<SymbolId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    reference_signatures: &BTreeMap<SymbolIdentity, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
) -> MirEffectSummary {
    match instruction.kind() {
        MirInstructionKind::CallDirect { function, .. } => signatures
            .get(function)
            .map(|(_, _, effects)| *effects)
            .unwrap_or_default(),
        MirInstructionKind::CallReferenced { function, .. } => reference_signatures
            .get(function)
            .map(|(_, _, effects)| *effects)
            .unwrap_or_default(),
        MirInstructionKind::CallDirectMethod { method, .. } => method_signatures
            .get(method)
            .map(|(_, _, effects)| *effects)
            .unwrap_or_default(),
        MirInstructionKind::CallIndirect {
            declared_effects, ..
        } => *declared_effects,
        kind => local_instruction_effects(kind),
    }
}

fn verify_unwind_action(
    instruction: &MirInstruction,
    blocks: &BTreeMap<BlockId, &MirBlock>,
    errors: &mut Vec<MirVerificationError>,
) {
    let unwind = instruction.unwind_action();
    if let MirUnwindAction::Cleanup(target) = unwind {
        if !instruction.effects().contains(MirEffect::MayUnwind) {
            errors.push(MirVerificationError::InvalidUnwindAction {
                instruction: instruction.result(),
            });
            return;
        }
        let Some(cleanup) = blocks.get(&target) else {
            errors.push(MirVerificationError::InvalidUnwindAction {
                instruction: instruction.result(),
            });
            return;
        };
        if !cleanup.arguments().is_empty() {
            errors.push(MirVerificationError::InvalidUnwindAction {
                instruction: instruction.result(),
            });
        }
        if !cleanup
            .cleanup()
            .is_some_and(|cleanup| cleanup.reason() == MirCleanupExitReason::Unwind)
        {
            errors.push(MirVerificationError::InvalidCleanupBlock { block: target });
        }
    }
}

#[allow(clippy::too_many_lines)]
fn verify_gc_contracts(
    function: &MirFunction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    facts: &ControlFlowFacts<'_, '_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let expected_roots = expected_safe_point_roots(function, arena);
    for block in &function.blocks {
        let mut straight_line_work = 0_usize;
        for (index, instruction) in block.instructions.iter().enumerate() {
            if straight_line_work >= MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS
                && !matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. })
            {
                errors.push(MirVerificationError::MissingGcSafePoint {
                    instruction: instruction.result(),
                });
                straight_line_work = 0;
            }
            let requires_safe_point = instruction.effects().contains(MirEffect::Allocates)
                || matches!(
                    instruction.kind(),
                    MirInstructionKind::CallDirect { .. }
                        | MirInstructionKind::CallDirectMethod { .. }
                        | MirInstructionKind::CallIndirect { .. }
                ) && instruction.effects().contains(MirEffect::GcSafePoint);
            if requires_safe_point
                && !index.checked_sub(1).is_some_and(|previous| {
                    matches!(
                        block.instructions()[previous].kind(),
                        MirInstructionKind::GcSafePoint { .. }
                    )
                })
            {
                errors.push(MirVerificationError::MissingGcSafePoint {
                    instruction: instruction.result(),
                });
            }
            match instruction.kind() {
                MirInstructionKind::ArrayMake { element_map, .. } => {
                    if *element_map != array_element_map(arena, instruction.result_type()) {
                        errors.push(MirVerificationError::InvalidObjectMap {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::TableMake {
                    key_map, value_map, ..
                } => {
                    if (*key_map, *value_map)
                        != table_element_maps(arena, instruction.result_type())
                    {
                        errors.push(MirVerificationError::InvalidObjectMap {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::ClassMake {
                    class, object_map, ..
                } => {
                    if schema.classes.get(class).is_some_and(|declaration| {
                        expected_class_object_map(declaration, arena) != *object_map
                    }) {
                        errors.push(MirVerificationError::InvalidObjectMap {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::TaskCreate {
                    dispatch,
                    arguments,
                    completion_type,
                    object_map,
                    ..
                } => {
                    let argument_types = arguments
                        .iter()
                        .map(|argument| facts.values.get(argument).copied())
                        .collect::<Option<Vec<_>>>();
                    if argument_types.is_some_and(|argument_types| {
                        task_object_map(dispatch, &argument_types, *completion_type, arena)
                            != *object_map
                    }) {
                        errors.push(MirVerificationError::InvalidObjectMap {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::GcSafePoint {
                    safe_point,
                    roots,
                    stack_map,
                } => {
                    let expected = expected_roots
                        .get(&instruction.result())
                        .cloned()
                        .unwrap_or_default();
                    if roots.len() != expected.len()
                        || stack_map.root_slots().len() != expected.len()
                        || stack_map.safe_point() != *safe_point
                    {
                        errors.push(MirVerificationError::IncompleteStackMap {
                            instruction: instruction.result(),
                            expected: expected.len(),
                            found: roots.len().min(stack_map.root_slots().len()),
                        });
                    }
                    for root in roots {
                        if !expected.contains(root)
                            || !facts.values.get(root).is_some_and(|type_id| {
                                is_managed_reference_type_id(*type_id, Some(arena))
                            })
                        {
                            errors.push(MirVerificationError::InvalidStackMapRoot {
                                instruction: instruction.result(),
                                root: *root,
                            });
                        }
                    }
                    for missing in expected.iter().filter(|root| !roots.contains(root)) {
                        errors.push(MirVerificationError::InvalidStackMapRoot {
                            instruction: instruction.result(),
                            root: *missing,
                        });
                    }
                }
                MirInstructionKind::RetainRoot { value } => {
                    if !facts
                        .values
                        .get(value)
                        .is_some_and(|type_id| is_managed_reference_type_id(*type_id, Some(arena)))
                    {
                        errors.push(MirVerificationError::InvalidStackMapRoot {
                            instruction: instruction.result(),
                            root: *value,
                        });
                    }
                }
                MirInstructionKind::ReleaseRoot { handle } => {
                    if !facts.root_handles.contains(handle) {
                        errors.push(MirVerificationError::ReleaseWithoutRetain {
                            instruction: instruction.result(),
                            value: *handle,
                        });
                    }
                }
                MirInstructionKind::Pin { value } => {
                    if !facts
                        .values
                        .get(value)
                        .is_some_and(|type_id| is_managed_reference_type_id(*type_id, Some(arena)))
                    {
                        errors.push(MirVerificationError::InvalidPinnedReference {
                            instruction: instruction.result(),
                            value: *value,
                        });
                    }
                }
                MirInstructionKind::Unpin { handle } => {
                    if !facts.pin_handles.contains(handle) {
                        errors.push(MirVerificationError::UnpinWithoutPin {
                            instruction: instruction.result(),
                            value: *handle,
                        });
                    }
                }
                MirInstructionKind::WriteBarrier {
                    owner,
                    slot,
                    previous,
                    value,
                } => {
                    verify_write_barrier(
                        instruction,
                        *owner,
                        *slot,
                        *previous,
                        *value,
                        arena,
                        schema,
                        facts.values,
                        errors,
                    );
                    let followed_by_matching_store = block
                        .instructions()
                        .get(index.saturating_add(1))
                        .is_some_and(|next| {
                            matches!(
                                (next.kind(), value),
                                (
                                    MirInstructionKind::FieldSet {
                                        base,
                                        value: stored,
                                        ..
                                    },
                                    Some(barrier_value),
                                ) if base == owner && stored == barrier_value
                            )
                        });
                    if !followed_by_matching_store {
                        errors.push(MirVerificationError::UnexpectedWriteBarrier {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::FieldSet { base, field, value } => verify_field_store_barrier(
                    instruction,
                    block,
                    index,
                    *base,
                    *field,
                    *value,
                    arena,
                    schema,
                    errors,
                ),
                _ => {}
            }
            if matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. }) {
                straight_line_work = 0;
            } else {
                straight_line_work = straight_line_work.saturating_add(1);
            }
        }
        let has_backedge = terminator_targets(block.terminator())
            .into_iter()
            .any(|target| target <= block.block());
        if has_backedge
            && !block.instructions().last().is_some_and(|instruction| {
                matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. })
            })
        {
            errors.push(MirVerificationError::MissingBackedgeSafePoint(
                block.block(),
            ));
        }
    }
    verify_root_balance(function, errors);
    verify_pin_balance(function, errors);
}

fn expected_class_object_map(declaration: &MirClassDeclaration, arena: &TypeArena) -> ObjectMap {
    let references = declaration
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, field)| is_managed_reference_type_id(field.field_type(), Some(arena)))
        .map(|(index, _)| ObjectSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
        .collect();
    ObjectMap::new(
        u32::try_from(declaration.fields().len()).unwrap_or(u32::MAX),
        references,
    )
    .expect("declared class fields form a canonical object map")
}

#[allow(clippy::too_many_arguments)]
fn verify_write_barrier(
    instruction: &MirInstruction,
    owner: ValueId,
    slot: ObjectSlot,
    previous: Option<ValueId>,
    value: Option<ValueId>,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(owner_type) = values.get(&owner).copied() else {
        return;
    };
    let valid_slot = schema.classes.values().any(|class| {
        class.type_id() == owner_type
            && expected_class_object_map(class, arena).is_reference_slot(slot)
    });
    let operands_are_references = previous.into_iter().chain(value).all(|operand| {
        values
            .get(&operand)
            .is_some_and(|type_id| is_managed_reference_type_id(*type_id, Some(arena)))
    });
    if !valid_slot || !operands_are_references {
        errors.push(MirVerificationError::UnexpectedWriteBarrier {
            instruction: instruction.result(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_field_store_barrier(
    instruction: &MirInstruction,
    block: &MirBlock,
    index: usize,
    base: ValueId,
    field: FieldId,
    value: ValueId,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declared) = schema.fields.get(&field) else {
        return;
    };
    if !is_managed_reference_type_id(declared.field_type, Some(arena)) {
        return;
    }
    let expected_slot = schema.classes.values().find_map(|class| {
        class
            .fields()
            .iter()
            .position(|candidate| candidate.field() == field)
            .map(|position| ObjectSlot::new(u32::try_from(position).unwrap_or(u32::MAX)))
    });
    let Some(previous_instruction) = index
        .checked_sub(1)
        .and_then(|previous| block.instructions().get(previous))
    else {
        errors.push(MirVerificationError::MissingWriteBarrier {
            instruction: instruction.result(),
            field,
        });
        return;
    };
    let valid = matches!(
        previous_instruction.kind(),
        MirInstructionKind::WriteBarrier {
            owner,
            slot,
            value: Some(stored),
            ..
        } if *owner == base && Some(*slot) == expected_slot && *stored == value
    );
    if !valid {
        errors.push(MirVerificationError::MissingWriteBarrier {
            instruction: instruction.result(),
            field,
        });
    }
}

fn verify_root_balance(function: &MirFunction, errors: &mut Vec<MirVerificationError>) {
    verify_handle_balance(function, HandleKind::Root, errors);
}

fn verify_pin_balance(function: &MirFunction, errors: &mut Vec<MirVerificationError>) {
    verify_handle_balance(function, HandleKind::Pin, errors);
}

#[derive(Clone, Copy)]
enum HandleKind {
    Root,
    Pin,
}

impl HandleKind {
    const fn acquires(self, instruction: &MirInstructionKind) -> bool {
        matches!(
            (self, instruction),
            (Self::Root, MirInstructionKind::RetainRoot { .. })
                | (Self::Pin, MirInstructionKind::Pin { .. })
        )
    }

    const fn released_handle(self, instruction: &MirInstructionKind) -> Option<ValueId> {
        match (self, instruction) {
            (Self::Root, MirInstructionKind::ReleaseRoot { handle })
            | (Self::Pin, MirInstructionKind::Unpin { handle }) => Some(*handle),
            _ => None,
        }
    }

    const fn release_without_acquire(
        self,
        instruction: ValueId,
        value: ValueId,
    ) -> MirVerificationError {
        match self {
            Self::Root => MirVerificationError::ReleaseWithoutRetain { instruction, value },
            Self::Pin => MirVerificationError::UnpinWithoutPin { instruction, value },
        }
    }

    const fn unreleased(self, block: BlockId, value: ValueId) -> MirVerificationError {
        match self {
            Self::Root => MirVerificationError::UnreleasedRoot { block, value },
            Self::Pin => MirVerificationError::UnreleasedPin { block, value },
        }
    }

    const fn state_mismatch(self, target: BlockId) -> MirVerificationError {
        match self {
            Self::Root => MirVerificationError::RootStateMismatch(target),
            Self::Pin => MirVerificationError::PinStateMismatch(target),
        }
    }
}

fn verify_handle_balance(
    function: &MirFunction,
    kind: HandleKind,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(entry) = function.blocks.first() else {
        return;
    };
    let blocks: BTreeMap<_, _> = function
        .blocks
        .iter()
        .map(|block| (block.block(), block))
        .collect();
    let mut incoming = BTreeMap::<BlockId, BTreeSet<ValueId>>::new();
    incoming.insert(entry.block(), BTreeSet::new());
    let mut pending = vec![entry.block()];
    while let Some(block_id) = pending.pop() {
        let Some(block) = blocks.get(&block_id).copied() else {
            continue;
        };
        let mut retained = incoming.get(&block_id).cloned().unwrap_or_default();
        for instruction in block.instructions() {
            if kind.acquires(instruction.kind()) {
                retained.insert(instruction.result());
            }
            if let Some(handle) = kind.released_handle(instruction.kind())
                && !retained.remove(&handle)
            {
                errors.push(kind.release_without_acquire(instruction.result(), handle));
            }
            let catches_unwind = instruction_unwind_target(instruction).is_some();
            let propagates_unwind =
                instruction.effects().contains(MirEffect::MayUnwind) && !catches_unwind;
            if instruction.effects().contains(MirEffect::MayTrap) || propagates_unwind {
                for value in &retained {
                    errors.push(kind.unreleased(block_id, *value));
                }
            }
            if let Some(target) = instruction_unwind_target(instruction) {
                merge_handle_state(target, &retained, &mut incoming, &mut pending, kind, errors);
            }
        }
        let targets = terminator_targets(block.terminator());
        if targets.is_empty() {
            for value in retained {
                errors.push(kind.unreleased(block_id, value));
            }
            continue;
        }
        for target in targets {
            let edge_state = match block.terminator() {
                MirTerminator::Branch { arguments, .. } => {
                    translate_handle_state(target, arguments, &retained, &blocks)
                }
                _ => retained.clone(),
            };
            merge_handle_state(
                target,
                &edge_state,
                &mut incoming,
                &mut pending,
                kind,
                errors,
            );
        }
    }
}

fn translate_handle_state(
    target: BlockId,
    arguments: &[ValueId],
    retained: &BTreeSet<ValueId>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeSet<ValueId> {
    let mut translated = retained.clone();
    let Some(target) = blocks.get(&target) else {
        return translated;
    };
    for (parameter, argument) in target.arguments().iter().zip(arguments) {
        if translated.remove(argument) {
            translated.insert(parameter.value());
        }
    }
    translated
}

fn merge_handle_state(
    target: BlockId,
    retained: &BTreeSet<ValueId>,
    incoming: &mut BTreeMap<BlockId, BTreeSet<ValueId>>,
    pending: &mut Vec<BlockId>,
    kind: HandleKind,
    errors: &mut Vec<MirVerificationError>,
) {
    match incoming.get(&target) {
        Some(existing) if existing != retained => {
            errors.push(kind.state_mismatch(target));
        }
        Some(_) => {}
        None => {
            incoming.insert(target, retained.clone());
            pending.push(target);
        }
    }
}

#[derive(Clone, Copy)]
struct DefinitionSite {
    block: BlockId,
    instruction: Option<usize>,
}

#[derive(Default)]
struct DefinitionTables {
    values: BTreeMap<ValueId, TypeId>,
    root_handles: BTreeSet<ValueId>,
    pin_handles: BTreeSet<ValueId>,
    sites: BTreeMap<ValueId, DefinitionSite>,
    seen: BTreeSet<ValueId>,
}

impl DefinitionTables {
    fn collect(
        &mut self,
        value: ValueId,
        type_id: TypeId,
        site: DefinitionSite,
        arena: &TypeArena,
        errors: &mut Vec<MirVerificationError>,
    ) {
        if !arena.is_valid_hir_type(type_id) {
            errors.push(MirVerificationError::InvalidType(type_id));
        }
        if !self.seen.insert(value) {
            errors.push(MirVerificationError::DuplicateValue(value));
            return;
        }
        self.values.insert(value, type_id);
        self.sites.insert(value, site);
    }

    fn collect_root_handle(
        &mut self,
        value: ValueId,
        site: DefinitionSite,
        errors: &mut Vec<MirVerificationError>,
    ) {
        if !self.seen.insert(value) {
            errors.push(MirVerificationError::DuplicateValue(value));
            return;
        }
        self.root_handles.insert(value);
        self.sites.insert(value, site);
    }

    fn collect_pin_handle(
        &mut self,
        value: ValueId,
        site: DefinitionSite,
        errors: &mut Vec<MirVerificationError>,
    ) {
        if !self.seen.insert(value) {
            errors.push(MirVerificationError::DuplicateValue(value));
            return;
        }
        self.pin_handles.insert(value);
        self.sites.insert(value, site);
    }
}

struct ControlFlowFacts<'facts, 'function> {
    values: &'facts BTreeMap<ValueId, TypeId>,
    root_handles: &'facts BTreeSet<ValueId>,
    pin_handles: &'facts BTreeSet<ValueId>,
    definitions: &'facts BTreeMap<ValueId, DefinitionSite>,
    dominators: &'facts BTreeMap<BlockId, BTreeSet<BlockId>>,
    blocks: &'facts BTreeMap<BlockId, &'function MirBlock>,
}

fn collect_blocks<'function>(
    function: &'function MirFunction,
    errors: &mut Vec<MirVerificationError>,
) -> BTreeMap<BlockId, &'function MirBlock> {
    let mut blocks = BTreeMap::new();
    for block in &function.blocks {
        if block.block.raw() as usize >= function.blocks.len() {
            errors.push(MirVerificationError::InvalidBlock(block.block));
        }
        if blocks.insert(block.block, block).is_some() {
            errors.push(MirVerificationError::DuplicateBlock(block.block));
        }
    }
    blocks
}

fn compute_dominators(
    function: &MirFunction,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeMap<BlockId, BTreeSet<BlockId>> {
    let Some(entry) = function.blocks.first().map(MirBlock::block) else {
        return BTreeMap::new();
    };
    let mut predecessors: BTreeMap<_, BTreeSet<_>> = blocks
        .keys()
        .map(|block| (*block, BTreeSet::new()))
        .collect();
    for block in &function.blocks {
        for target in block_targets(block) {
            if let Some(target_predecessors) = predecessors.get_mut(&target) {
                target_predecessors.insert(block.block());
            }
        }
    }
    let reachable = reachable_blocks(entry, blocks);
    let mut dominators: BTreeMap<_, _> = blocks
        .keys()
        .map(|block| {
            let initial = if *block == entry || !reachable.contains(block) {
                BTreeSet::from([*block])
            } else {
                reachable.clone()
            };
            (*block, initial)
        })
        .collect();
    loop {
        let mut changed = false;
        for block in reachable.iter().copied().filter(|block| *block != entry) {
            let mut incoming = predecessors[&block]
                .iter()
                .filter(|predecessor| reachable.contains(predecessor))
                .map(|predecessor| dominators[predecessor].clone());
            let mut next = incoming.next().unwrap_or_default();
            for predecessor_dominators in incoming {
                next = next
                    .intersection(&predecessor_dominators)
                    .copied()
                    .collect();
            }
            next.insert(block);
            if dominators[&block] != next {
                dominators.insert(block, next);
                changed = true;
            }
        }
        if !changed {
            return dominators;
        }
    }
}

fn compute_optional_presence_facts(
    function: &MirFunction,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeMap<BlockId, BTreeSet<ValueId>> {
    let Some(entry) = function.blocks.first().map(MirBlock::block) else {
        return BTreeMap::new();
    };
    let reachable = reachable_blocks(entry, blocks);
    let mut conditions = BTreeMap::new();
    let mut all_optionals = BTreeSet::new();
    for block in &function.blocks {
        for instruction in block.instructions() {
            if let MirInstructionKind::OptionalIsPresent { optional } = instruction.kind() {
                conditions.insert(instruction.result(), (*optional, true));
                all_optionals.insert(*optional);
            }
        }
    }
    for block in &function.blocks {
        for instruction in block.instructions() {
            if let MirInstructionKind::BooleanNot { operand } = instruction.kind()
                && let Some((optional, present_when_true)) = conditions.get(operand).copied()
            {
                conditions.insert(instruction.result(), (optional, !present_when_true));
            }
        }
    }

    let mut predecessors: BTreeMap<BlockId, Vec<BlockId>> =
        blocks.keys().map(|block| (*block, Vec::new())).collect();
    for block in &function.blocks {
        for target in block_targets(block) {
            if let Some(incoming) = predecessors.get_mut(&target) {
                incoming.push(block.block());
            }
        }
    }
    let mut facts: BTreeMap<BlockId, BTreeSet<ValueId>> = blocks
        .keys()
        .map(|block| {
            let initial = if *block == entry || !reachable.contains(block) {
                BTreeSet::new()
            } else {
                all_optionals.clone()
            };
            (*block, initial)
        })
        .collect();

    loop {
        let mut changed = false;
        for block in reachable.iter().copied().filter(|block| *block != entry) {
            let mut incoming = predecessors[&block]
                .iter()
                .filter(|predecessor| reachable.contains(predecessor))
                .map(|predecessor| {
                    optional_edge_facts(
                        &facts[predecessor],
                        blocks[predecessor].terminator(),
                        block,
                        &conditions,
                    )
                });
            let mut next = incoming.next().unwrap_or_default();
            for predecessor in incoming {
                next = next.intersection(&predecessor).copied().collect();
            }
            if facts[&block] != next {
                facts.insert(block, next);
                changed = true;
            }
        }
        if !changed {
            return facts;
        }
    }
}

fn optional_edge_facts(
    incoming: &BTreeSet<ValueId>,
    terminator: &MirTerminator,
    target: BlockId,
    conditions: &BTreeMap<ValueId, (ValueId, bool)>,
) -> BTreeSet<ValueId> {
    let mut facts = incoming.clone();
    let MirTerminator::ConditionalBranch {
        condition,
        when_true,
        when_false,
    } = terminator
    else {
        return facts;
    };
    let Some((optional, present_when_true)) = conditions.get(condition).copied() else {
        return facts;
    };
    if when_true == when_false {
        facts.remove(&optional);
    } else if target == *when_true {
        if present_when_true {
            facts.insert(optional);
        } else {
            facts.remove(&optional);
        }
    } else if target == *when_false {
        if present_when_true {
            facts.remove(&optional);
        } else {
            facts.insert(optional);
        }
    }
    facts
}

fn reachable_blocks(entry: BlockId, blocks: &BTreeMap<BlockId, &MirBlock>) -> BTreeSet<BlockId> {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![entry];
    while let Some(block) = pending.pop() {
        if !reachable.insert(block) {
            continue;
        }
        if let Some(block) = blocks.get(&block) {
            pending.extend(
                block_targets(block)
                    .into_iter()
                    .filter(|target| blocks.contains_key(target)),
            );
        }
    }
    reachable
}

pub(crate) fn terminator_targets(terminator: &MirTerminator) -> Vec<BlockId> {
    match terminator {
        MirTerminator::Branch { target, .. } => vec![*target],
        MirTerminator::ConditionalBranch {
            when_true,
            when_false,
            ..
        } => vec![*when_true, *when_false],
        MirTerminator::UnionSwitch { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
        MirTerminator::ErrorSwitch { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
        MirTerminator::Suspend {
            resume,
            cancellation,
            unwind,
            ..
        } => {
            let mut targets = vec![*resume, *cancellation];
            if let MirUnwindAction::Cleanup(target) = unwind {
                targets.push(*target);
            }
            targets
        }
        MirTerminator::Missing
        | MirTerminator::Return { .. }
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::ResumeUnwind
        | MirTerminator::Unreachable => Vec::new(),
    }
}

pub(crate) fn terminator_operands(terminator: &MirTerminator) -> Vec<ValueId> {
    match terminator {
        MirTerminator::Return { values } => values.clone(),
        MirTerminator::ConditionalBranch { condition, .. } => vec![*condition],
        MirTerminator::UnionSwitch { scrutinee, .. } => vec![*scrutinee],
        MirTerminator::ErrorSwitch { scrutinee, .. } => vec![*scrutinee],
        MirTerminator::Suspend { operation, .. } => match operation {
            MirSuspendOperation::Task { task, .. } => vec![*task],
        },
        MirTerminator::Missing
        | MirTerminator::Branch { .. }
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::ResumeUnwind
        | MirTerminator::Unreachable => Vec::new(),
    }
}

pub(crate) fn instruction_unwind_target(instruction: &MirInstruction) -> Option<BlockId> {
    match instruction.unwind_action() {
        MirUnwindAction::Cleanup(target) => Some(target),
        MirUnwindAction::Propagate => None,
    }
}

pub(crate) fn block_targets(block: &MirBlock) -> Vec<BlockId> {
    let mut targets = terminator_targets(&block.terminator);
    targets.extend(
        block
            .instructions
            .iter()
            .filter_map(instruction_unwind_target),
    );
    targets.sort_unstable();
    targets.dedup();
    targets
}

#[derive(Clone, Copy)]
struct CallableSignatures<'a> {
    functions: &'a BTreeMap<SymbolId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    references: &'a BTreeMap<SymbolIdentity, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    methods: &'a BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    async_functions: &'a BTreeSet<SymbolId>,
    async_references: &'a BTreeSet<SymbolIdentity>,
}

fn verify_instruction_types(
    instruction: &MirInstruction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    signatures: CallableSignatures<'_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let requires_effect_form = matches!(
        instruction.kind(),
        MirInstructionKind::GcSafePoint { .. }
            | MirInstructionKind::RetainRoot { .. }
            | MirInstructionKind::ReleaseRoot { .. }
            | MirInstructionKind::Pin { .. }
            | MirInstructionKind::Unpin { .. }
            | MirInstructionKind::WriteBarrier { .. }
    );
    if requires_effect_form && instruction.has_result() {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
        return;
    }
    if verify_numeric_instruction(instruction, arena, values, errors) {
        return;
    }
    if verify_iteration_instruction(instruction, arena, values, errors) {
        return;
    }
    if verify_schema_instruction(instruction, arena, schema, values, errors) {
        return;
    }
    if verify_callable_instruction(instruction, arena, schema, values, signatures, errors) {
        return;
    }
    match instruction.kind() {
        MirInstructionKind::OptionalIsPresent { optional } => {
            let valid_operand = values
                .get(optional)
                .copied()
                .and_then(|type_id| optional_inner_type(arena, type_id))
                .is_some();
            if !valid_operand || arena.source_type("Boolean") != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::OptionalGet { optional } => {
            let valid = values
                .get(optional)
                .copied()
                .and_then(|type_id| optional_inner_type(arena, type_id))
                == Some(instruction.result_type());
            if !valid {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ResultMake {
            result,
            case,
            arguments,
        } => {
            let expected = match arena.get(instruction.result_type()) {
                Some(SemanticType::Builtin {
                    definition,
                    arguments: types,
                }) if definition == result => usize::try_from(case.raw())
                    .ok()
                    .and_then(|index| types.get(index))
                    .copied(),
                _ => None,
            };
            let valid = arguments.len() == 1
                && expected.is_some_and(|expected| values.get(&arguments[0]) == Some(&expected));
            if !valid {
                errors.push(MirVerificationError::InvalidResultOperation {
                    instruction: instruction.result(),
                });
            }
        }
        MirInstructionKind::IterationMake {
            iteration,
            case,
            arguments,
        } => {
            let expected_item = match arena.get(instruction.result_type()) {
                Some(SemanticType::Builtin {
                    definition,
                    arguments: types,
                }) if definition == iteration && types.len() == 1 => Some(types[0]),
                _ => None,
            };
            let valid = (case.raw() == 0
                && arguments.len() == 1
                && expected_item
                    .is_some_and(|expected| values.get(&arguments[0]) == Some(&expected)))
                || (case.raw() == 1 && arguments.is_empty() && expected_item.is_some());
            if !valid {
                errors.push(MirVerificationError::InvalidIterationOperation {
                    instruction: instruction.result(),
                });
            }
        }
        MirInstructionKind::ResultIsOk { result, definition } => {
            let valid = values.get(result).is_some_and(|type_id| {
                matches!(arena.get(*type_id), Some(SemanticType::Builtin { definition: found, arguments }) if found == definition && arguments.len() == 2)
            }) && arena.source_type("Boolean") == Some(instruction.result_type());
            if !valid {
                errors.push(MirVerificationError::InvalidResultOperation {
                    instruction: instruction.result(),
                });
            }
        }
        MirInstructionKind::ResultGetOk { result, definition }
        | MirInstructionKind::ResultGetError { result, definition } => {
            let index = usize::from(matches!(
                instruction.kind(),
                MirInstructionKind::ResultGetError { .. }
            ));
            let expected = values
                .get(result)
                .and_then(|type_id| match arena.get(*type_id) {
                    Some(SemanticType::Builtin {
                        definition: found,
                        arguments,
                    }) if found == definition && arguments.len() == 2 => {
                        arguments.get(index).copied()
                    }
                    _ => None,
                });
            if expected != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidResultOperation {
                    instruction: instruction.result(),
                });
            }
        }
        MirInstructionKind::StringConcat { left, right } => {
            let Some(string) = arena.source_type("String") else {
                return;
            };
            verify_operand_type(instruction.result(), *left, string, values, errors);
            verify_operand_type(instruction.result(), *right, string, values, errors);
            if instruction.result_type() != string {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::StringFormat { kind, value } => {
            let expected = match kind {
                pop_types::StringFormatKind::Boolean => arena.source_type("Boolean"),
                pop_types::StringFormatKind::Integer(kind) => integer_type(arena, *kind),
                pop_types::StringFormatKind::Float(kind) => float_type(arena, *kind),
            };
            if let Some(expected) = expected {
                verify_operand_type(instruction.result(), *value, expected, values, errors);
            }
            if arena.source_type("String") != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::CompareEqual { left, right }
        | MirInstructionKind::CompareNotEqual { left, right } => {
            verify_equality_instruction(instruction, *left, *right, arena, values, errors);
        }
        MirInstructionKind::TupleMake(elements) => {
            let Some(SemanticType::Tuple(element_types)) = arena.get(instruction.result_type())
            else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            if elements.len() != element_types.len() {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            }
            for (element, expected) in elements.iter().zip(element_types) {
                verify_operand_type(instruction.result(), *element, *expected, values, errors);
            }
        }
        MirInstructionKind::TupleGet { tuple, index } => {
            let Some(tuple_type) = values.get(tuple).copied() else {
                return;
            };
            let Some(SemanticType::Tuple(element_types)) = arena.get(tuple_type) else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *tuple,
                    found: tuple_type,
                });
                return;
            };
            if element_types.get(*index as usize) != Some(&instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayMake { elements, .. } => {
            let Some(SemanticType::Array(element_type)) =
                arena.get(instruction.result_type()).cloned()
            else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            for operand in elements {
                verify_operand_type(instruction.result(), *operand, element_type, values, errors);
            }
        }
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            element_map,
        } => {
            let Some(SemanticType::Array(element_type)) =
                arena.get(instruction.result_type()).cloned()
            else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *length, integer, values, errors);
            }
            verify_operand_type(
                instruction.result(),
                *initial_value,
                element_type,
                values,
                errors,
            );
            if *element_map != array_element_map(arena, instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::TableMake { entries, .. } => {
            let Some(SemanticType::Table { key, value }) =
                arena.get(instruction.result_type()).cloned()
            else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            for (entry_key, entry_value) in entries {
                verify_operand_type(instruction.result(), *entry_key, key, values, errors);
                verify_operand_type(instruction.result(), *entry_value, value, values, errors);
            }
        }
        MirInstructionKind::TableGet { table, key } => {
            let Some(table_type) = values.get(table).copied() else {
                return;
            };
            let Some(SemanticType::Table {
                key: key_type,
                value: value_type,
            }) = arena.get(table_type).cloned()
            else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *table,
                    found: table_type,
                });
                return;
            };
            verify_operand_type(instruction.result(), *key, key_type, values, errors);
            if !is_optional_of(arena, instruction.result_type(), value_type) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::TableSet {
            table,
            key,
            value,
            key_map,
            value_map,
        } => {
            let Some(table_type) = values.get(table).copied() else {
                return;
            };
            let Some(SemanticType::Table {
                key: key_type,
                value: value_type,
            }) = arena.get(table_type).cloned()
            else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *table,
                    found: table_type,
                });
                return;
            };
            verify_operand_type(instruction.result(), *key, key_type, values, errors);
            verify_operand_type(instruction.result(), *value, value_type, values, errors);
            if (*key_map, *value_map) != table_element_maps(arena, table_type)
                || arena.source_type("nil") != Some(instruction.result_type())
            {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayGet { array, index } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *index, integer, values, errors);
            }
            if !is_optional_of(arena, instruction.result_type(), element_type) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayLength { array } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            if !matches!(arena.get(array_type), Some(SemanticType::Array(_))) {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
            }
            if arena.source_type("Int") != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayGetChecked { array, index } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *index, integer, values, errors);
            }
            if instruction.result_type() != element_type {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            element_map,
        } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *index, integer, values, errors);
            }
            verify_operand_type(instruction.result(), *value, element_type, values, errors);
            if *element_map != array_element_map(arena, array_type) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
            if arena.source_type("nil") != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayFill {
            array,
            value,
            element_map,
        } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            verify_operand_type(instruction.result(), *value, element_type, values, errors);
            if *element_map != array_element_map(arena, array_type)
                || arena.source_type("nil") != Some(instruction.result_type())
            {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ListCreate {
            capacity,
            element_map,
        } => {
            let Some(_) = list_element_type(arena, instruction.result_type()) else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            if let (Some(capacity), Some(integer)) = (capacity, arena.source_type("Int")) {
                verify_operand_type(instruction.result(), *capacity, integer, values, errors);
            }
            if *element_map != list_element_map(arena, instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ListLength { list } => {
            let Some(list_type) = values.get(list).copied() else {
                return;
            };
            if list_element_type(arena, list_type).is_none() {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *list,
                    found: list_type,
                });
            }
            if arena.source_type("Int") != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ListGet { list, index }
        | MirInstructionKind::ListGetChecked { list, index } => {
            let Some(list_type) = values.get(list).copied() else {
                return;
            };
            let Some(element_type) = list_element_type(arena, list_type) else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *list,
                    found: list_type,
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *index, integer, values, errors);
            }
            let valid = if matches!(instruction.kind(), MirInstructionKind::ListGet { .. }) {
                is_optional_of(arena, instruction.result_type(), element_type)
            } else {
                instruction.result_type() == element_type
            };
            if !valid {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ListSet {
            list,
            index,
            value,
            element_map,
        } => {
            verify_list_mutation(
                instruction,
                *list,
                Some(*index),
                *value,
                *element_map,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::ListAdd {
            list,
            value,
            element_map,
        } => {
            verify_list_mutation(
                instruction,
                *list,
                None,
                *value,
                *element_map,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::RangeCreate { first, last, step } => {
            let Some(first_type) = values.get(first).copied() else {
                return;
            };
            let valid_result = range_element_type(arena, instruction.result_type())
                .is_some_and(|element| element == first_type)
                && matches!(
                    arena.get(first_type),
                    Some(SemanticType::Primitive(pop_types::PrimitiveType::Integer(
                        _
                    )))
                );
            if !valid_result {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
            verify_operand_type(instruction.result(), *last, first_type, values, errors);
            verify_operand_type(instruction.result(), *step, first_type, values, errors);
        }
        _ => {}
    }
}

fn list_element_type(arena: &TypeArena, type_id: TypeId) -> Option<TypeId> {
    let list = embedded_bootstrap_schema()
        .ok()?
        .iteration_protocol()?
        .list();
    match arena.get(type_id)? {
        SemanticType::Builtin {
            definition,
            arguments,
        } if *definition == list && arguments.len() == 1 => Some(arguments[0]),
        _ => None,
    }
}

fn range_element_type(arena: &TypeArena, type_id: TypeId) -> Option<TypeId> {
    let range = embedded_bootstrap_schema()
        .ok()?
        .iteration_protocol()?
        .range();
    match arena.get(type_id)? {
        SemanticType::Builtin {
            definition,
            arguments,
        } if *definition == range && arguments.len() == 1 => Some(arguments[0]),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_list_mutation(
    instruction: &MirInstruction,
    list: ValueId,
    index: Option<ValueId>,
    value: ValueId,
    element_map: ArrayElementMap,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(list_type) = values.get(&list).copied() else {
        return;
    };
    let Some(element_type) = list_element_type(arena, list_type) else {
        errors.push(MirVerificationError::InvalidCollectionOperand {
            instruction: instruction.result(),
            operand: list,
            found: list_type,
        });
        return;
    };
    if let (Some(index), Some(integer)) = (index, arena.source_type("Int")) {
        verify_operand_type(instruction.result(), index, integer, values, errors);
    }
    verify_operand_type(instruction.result(), value, element_type, values, errors);
    if element_map != list_element_map(arena, list_type)
        || arena.source_type("nil") != Some(instruction.result_type())
    {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_iteration_instruction(
    instruction: &MirInstruction,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) -> bool {
    let kind = instruction.kind();
    if !matches!(
        kind,
        MirInstructionKind::CallBuiltinInterface { .. }
            | MirInstructionKind::IterationIsItem { .. }
            | MirInstructionKind::IterationGetItem { .. }
    ) {
        return false;
    }
    let protocol = embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol());
    let valid = protocol.is_some_and(|protocol| match kind {
        MirInstructionKind::CallBuiltinInterface {
            interface,
            method,
            arguments,
            ..
        } if arguments.len() == 1 && *method == protocol.iterator_method() => {
            let result_item =
                builtin_argument(arena, instruction.result_type(), protocol.iterator());
            let source_type = values.get(&arguments[0]).copied();
            result_item.is_some_and(|item| {
                (*interface == protocol.iterable()
                    && source_type
                        .and_then(|source| iteration_source_item(arena, source, protocol))
                        == Some(item))
                    || (*interface == protocol.iterator()
                        && source_type.and_then(|source| {
                            builtin_argument(arena, source, protocol.iterator())
                        }) == Some(item))
            })
        }
        MirInstructionKind::CallBuiltinInterface {
            interface,
            method,
            arguments,
            ..
        } if arguments.len() == 1 && *method == protocol.next_method() => {
            let source_item = values
                .get(&arguments[0])
                .and_then(|source| builtin_argument(arena, *source, protocol.iterator()));
            let result_item =
                builtin_argument(arena, instruction.result_type(), protocol.iteration());
            *interface == protocol.iterator() && source_item.is_some() && source_item == result_item
        }
        MirInstructionKind::IterationIsItem {
            iteration,
            definition,
            item_case,
            end_case,
        } => {
            values.get(iteration).is_some_and(|type_id| {
                builtin_argument(arena, *type_id, protocol.iteration()).is_some()
            }) && *definition == protocol.iteration()
                && *item_case == protocol.item_case()
                && *end_case == protocol.end_case()
                && arena.source_type("Boolean") == Some(instruction.result_type())
        }
        MirInstructionKind::IterationGetItem {
            iteration,
            definition,
            item_case,
        } => {
            let expected = values
                .get(iteration)
                .and_then(|type_id| builtin_argument(arena, *type_id, protocol.iteration()));
            *definition == protocol.iteration()
                && *item_case == protocol.item_case()
                && expected == Some(instruction.result_type())
        }
        _ => false,
    });
    if !valid {
        errors.push(MirVerificationError::InvalidIterationOperation {
            instruction: instruction.result(),
        });
    }
    true
}

fn builtin_argument(
    arena: &TypeArena,
    type_id: TypeId,
    definition: pop_foundation::BuiltinTypeId,
) -> Option<TypeId> {
    match arena.get(type_id) {
        Some(SemanticType::Builtin {
            definition: actual,
            arguments,
        }) if *actual == definition && arguments.len() == 1 => arguments.first().copied(),
        _ => None,
    }
}

fn iteration_source_item(
    arena: &TypeArena,
    type_id: TypeId,
    protocol: pop_types::BootstrapIterationProtocol,
) -> Option<TypeId> {
    match arena.get(type_id) {
        Some(SemanticType::Array(item)) => Some(*item),
        Some(SemanticType::Table { key, value }) => {
            arena.find(&SemanticType::Tuple(vec![*key, *value]))
        }
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) if arguments.len() == 1
            && (*definition == protocol.list()
                || *definition == protocol.range()
                || *definition == protocol.iterable()
                || *definition == protocol.iterator()) =>
        {
            arguments.first().copied()
        }
        _ => None,
    }
}

fn verify_numeric_instruction(
    instruction: &MirInstruction,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) -> bool {
    match instruction.kind() {
        MirInstructionKind::IntegerConstant(value) => {
            verify_numeric_result(instruction, integer_type(arena, value.kind()), errors);
        }
        MirInstructionKind::FloatConstant(value) => {
            verify_numeric_result(instruction, float_type(arena, value.kind()), errors);
        }
        MirInstructionKind::CheckedIntegerAdd { kind, left, right }
        | MirInstructionKind::CheckedIntegerSubtract { kind, left, right }
        | MirInstructionKind::CheckedIntegerMultiply { kind, left, right }
        | MirInstructionKind::CheckedIntegerDivide { kind, left, right }
        | MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                integer_type(arena, *kind),
                false,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::FloatAdd { kind, left, right }
        | MirInstructionKind::FloatSubtract { kind, left, right }
        | MirInstructionKind::FloatMultiply { kind, left, right }
        | MirInstructionKind::FloatDivide { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                float_type(arena, *kind),
                false,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::IntegerNegate { kind, operand } => {
            verify_numeric_unary(
                instruction,
                *operand,
                integer_type(arena, *kind),
                values,
                errors,
            );
        }
        MirInstructionKind::FloatNegate { kind, operand } => {
            verify_numeric_unary(
                instruction,
                *operand,
                float_type(arena, *kind),
                values,
                errors,
            );
        }
        MirInstructionKind::ConvertInteger {
            source,
            target,
            operand,
        } => verify_numeric_conversion(
            instruction,
            *operand,
            integer_type(arena, *source),
            integer_type(arena, *target),
            values,
            errors,
        ),
        MirInstructionKind::ConvertIntegerToFloat {
            source,
            target,
            operand,
        } => verify_numeric_conversion(
            instruction,
            *operand,
            integer_type(arena, *source),
            float_type(arena, *target),
            values,
            errors,
        ),
        MirInstructionKind::ConvertFloatToInteger {
            source,
            target,
            operand,
        } => verify_numeric_conversion(
            instruction,
            *operand,
            float_type(arena, *source),
            integer_type(arena, *target),
            values,
            errors,
        ),
        MirInstructionKind::ConvertFloat {
            source,
            target,
            operand,
        } => verify_numeric_conversion(
            instruction,
            *operand,
            float_type(arena, *source),
            float_type(arena, *target),
            values,
            errors,
        ),
        MirInstructionKind::CompareIntegerLess { kind, left, right }
        | MirInstructionKind::CompareIntegerLessOrEqual { kind, left, right }
        | MirInstructionKind::CompareIntegerGreater { kind, left, right }
        | MirInstructionKind::CompareIntegerGreaterOrEqual { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                integer_type(arena, *kind),
                true,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::CompareFloatLess { kind, left, right }
        | MirInstructionKind::CompareFloatLessOrEqual { kind, left, right }
        | MirInstructionKind::CompareFloatGreater { kind, left, right }
        | MirInstructionKind::CompareFloatGreaterOrEqual { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                float_type(arena, *kind),
                true,
                arena,
                values,
                errors,
            );
        }
        _ => return false,
    }
    true
}

fn verify_numeric_conversion(
    instruction: &MirInstruction,
    operand: ValueId,
    source: Option<TypeId>,
    target: Option<TypeId>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some((source, target)) = source.zip(target) else {
        if let Some(result_type) = instruction.optional_result_type() {
            errors.push(MirVerificationError::InvalidInstructionType {
                instruction: instruction.result(),
                result_type,
            });
        }
        return;
    };
    verify_operand_type(instruction.result(), operand, source, values, errors);
    verify_numeric_result(instruction, Some(target), errors);
}

fn verify_numeric_binary(
    instruction: &MirInstruction,
    operands: (ValueId, ValueId),
    operand_type: Option<TypeId>,
    comparison: bool,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(operand_type) = operand_type else {
        if let Some(result_type) = instruction.optional_result_type() {
            errors.push(MirVerificationError::InvalidInstructionType {
                instruction: instruction.result(),
                result_type,
            });
        }
        return;
    };
    verify_operand_type(
        instruction.result(),
        operands.0,
        operand_type,
        values,
        errors,
    );
    verify_operand_type(
        instruction.result(),
        operands.1,
        operand_type,
        values,
        errors,
    );
    let expected_result = if comparison {
        arena.source_type("Boolean")
    } else {
        Some(operand_type)
    };
    verify_numeric_result(instruction, expected_result, errors);
}

fn verify_numeric_unary(
    instruction: &MirInstruction,
    operand: ValueId,
    operand_type: Option<TypeId>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(operand_type) = operand_type else {
        if let Some(result_type) = instruction.optional_result_type() {
            errors.push(MirVerificationError::InvalidInstructionType {
                instruction: instruction.result(),
                result_type,
            });
        }
        return;
    };
    verify_operand_type(instruction.result(), operand, operand_type, values, errors);
    verify_numeric_result(instruction, Some(operand_type), errors);
}

fn verify_numeric_result(
    instruction: &MirInstruction,
    expected: Option<TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    if let (Some(found), Some(expected)) = (instruction.optional_result_type(), expected)
        && found != expected
    {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: found,
        });
    }
}

fn integer_type(arena: &TypeArena, kind: IntegerKind) -> Option<TypeId> {
    arena.source_type(integer_kind_text(kind))
}

fn float_type(arena: &TypeArena, kind: FloatKind) -> Option<TypeId> {
    arena.source_type(float_kind_text(kind))
}

fn verify_equality_instruction(
    instruction: &MirInstruction,
    left: ValueId,
    right: ValueId,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some((left_type, right_type)) = values.get(&left).copied().zip(values.get(&right).copied())
    else {
        return;
    };
    if arena.source_type("Boolean") != Some(instruction.result_type()) {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
    if !mir_equality_comparable(arena, left_type, right_type) {
        errors.push(MirVerificationError::InvalidComparisonOperands {
            instruction: instruction.result(),
            left: left_type,
            right: right_type,
        });
    }
}

fn mir_equality_comparable(arena: &TypeArena, left: TypeId, right: TypeId) -> bool {
    left == right && mir_supports_default_equality(arena, left)
}

fn mir_supports_default_equality(arena: &TypeArena, type_id: TypeId) -> bool {
    match arena.get(type_id) {
        Some(
            SemanticType::Primitive(
                pop_types::PrimitiveType::Nil
                | pop_types::PrimitiveType::Boolean
                | pop_types::PrimitiveType::Integer(_)
                | pop_types::PrimitiveType::String,
            )
            | SemanticType::Class { .. }
            | SemanticType::Enum { .. },
        ) => true,
        Some(SemanticType::Tuple(elements) | SemanticType::Union(elements)) => elements
            .iter()
            .all(|element| mir_supports_default_equality(arena, *element)),
        Some(SemanticType::Record(fields)) => fields
            .iter()
            .all(|(_, field_type)| mir_supports_default_equality(arena, *field_type)),
        _ => false,
    }
}

fn verify_schema_instruction(
    instruction: &MirInstruction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) -> bool {
    match instruction.kind() {
        MirInstructionKind::EnumConstant {
            definition,
            case,
            discriminant,
        } => {
            let valid = schema.enums.get(definition).is_some_and(|enumeration| {
                enumeration.type_id == instruction.result_type()
                    && enumeration.cases.iter().any(|candidate| {
                        candidate.case == *case && candidate.discriminant == *discriminant
                    })
            });
            if !valid {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::RecordMake { record, fields } => {
            let Some(declaration) = schema.records.get(record) else {
                errors.push(MirVerificationError::UnknownRecord {
                    instruction: instruction.result(),
                    record: *record,
                });
                return true;
            };
            verify_aggregate_result(instruction, declaration.type_id, errors);
            verify_constructed_fields(
                instruction,
                fields,
                &declaration.fields,
                true,
                values,
                errors,
            );
        }
        MirInstructionKind::ClassMake { class, fields, .. } => {
            let Some(declaration) = schema.classes.get(class) else {
                errors.push(MirVerificationError::UnknownClass {
                    instruction: instruction.result(),
                    class: *class,
                });
                return true;
            };
            verify_aggregate_result(instruction, declaration.type_id, errors);
            verify_constructed_fields(
                instruction,
                fields,
                &declaration.fields,
                true,
                values,
                errors,
            );
        }
        MirInstructionKind::RecordUpdate {
            record,
            base,
            fields,
        } => {
            let Some(declaration) = schema.records.get(record) else {
                errors.push(MirVerificationError::UnknownRecord {
                    instruction: instruction.result(),
                    record: *record,
                });
                return true;
            };
            verify_aggregate_result(instruction, declaration.type_id, errors);
            verify_operand_type(
                instruction.result(),
                *base,
                declaration.type_id,
                values,
                errors,
            );
            verify_constructed_fields(
                instruction,
                fields,
                &declaration.fields,
                false,
                values,
                errors,
            );
        }
        MirInstructionKind::FieldGet { base, field } => {
            verify_field_get(instruction, *base, *field, schema, values, errors);
        }
        MirInstructionKind::FieldSet { base, field, value } => {
            verify_field_set(
                instruction,
                FieldSetOperands {
                    base: *base,
                    field: *field,
                    value: *value,
                },
                arena,
                schema,
                values,
                errors,
            );
        }
        MirInstructionKind::UnionMake {
            union,
            case,
            arguments,
        } => verify_union_make(
            instruction,
            *union,
            *case,
            arguments,
            schema,
            values,
            errors,
        ),
        MirInstructionKind::ErrorMake {
            error,
            case,
            arguments,
        } => {
            let declaration = schema.errors.get(error);
            let expected = declaration.and_then(|declaration| {
                declaration
                    .cases()
                    .iter()
                    .find(|candidate| candidate.case() == *case)
            });
            let valid_type = matches!(
                arena.get(instruction.result_type()),
                Some(SemanticType::ErrorUnion { definition, .. }) if *definition == *error
            );
            let valid_arguments = expected.is_some_and(|case| {
                case.parameters().len() == arguments.len()
                    && case
                        .parameters()
                        .iter()
                        .zip(arguments)
                        .all(|(expected, argument)| values.get(argument) == Some(expected))
            });
            if !valid_type || !valid_arguments {
                errors.push(MirVerificationError::InvalidErrorOperation {
                    instruction: instruction.result(),
                    error: *error,
                });
            }
        }
        MirInstructionKind::InterfaceUpcast { value, interface } => {
            verify_interface_upcast(
                instruction,
                *value,
                *interface,
                arena,
                schema,
                values,
                errors,
            );
        }
        _ => return false,
    }
    true
}

fn verify_interface_upcast(
    instruction: &MirInstruction,
    value: ValueId,
    interface: NominalInterfaceId,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(source) = values.get(&value).copied() else {
        return;
    };
    let target = instruction.result_type();
    let class = match arena.get(source) {
        Some(SemanticType::Class { class, .. }) => schema.classes.get(class),
        _ => None,
    };
    let valid = match interface {
        NominalInterfaceId::User(interface) => class.is_some_and(|class| {
            class.interfaces().iter().any(|implementation| {
                implementation.interface() == interface && implementation.interface_type() == target
            })
        }),
        NominalInterfaceId::Builtin(interface) => class.is_some_and(|class| {
            class.builtin_interfaces().iter().any(|implementation| {
                implementation.interface() == interface && implementation.interface_type() == target
            })
        }),
    };
    if !valid {
        errors.push(MirVerificationError::InvalidInterfaceUpcast {
            instruction: instruction.result(),
            interface,
            source,
            target,
        });
    }
}

#[derive(Clone, Copy)]
struct FieldSetOperands {
    base: ValueId,
    field: FieldId,
    value: ValueId,
}

fn verify_aggregate_result(
    instruction: &MirInstruction,
    expected: TypeId,
    errors: &mut Vec<MirVerificationError>,
) {
    if instruction.result_type() != expected {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_constructed_fields(
    instruction: &MirInstruction,
    fields: &[(FieldId, ValueId)],
    declared: &[MirField],
    require_complete: bool,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let mut seen = BTreeSet::new();
    for (field, value) in fields {
        if !seen.insert(*field) {
            errors.push(MirVerificationError::DuplicateDeclaredField(*field));
        }
        let Some(declared) = declared.iter().find(|candidate| candidate.field == *field) else {
            errors.push(MirVerificationError::UnknownField {
                instruction: instruction.result(),
                field: *field,
            });
            continue;
        };
        verify_operand_type(
            instruction.result(),
            *value,
            declared.field_type,
            values,
            errors,
        );
    }
    if require_complete {
        for field in declared {
            if !seen.contains(&field.field) {
                errors.push(MirVerificationError::MissingDeclaredField {
                    instruction: instruction.result(),
                    field: field.field,
                });
            }
        }
    }
}

fn verify_field_get(
    instruction: &MirInstruction,
    base: ValueId,
    field: FieldId,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declared) = schema.fields.get(&field) else {
        errors.push(MirVerificationError::UnknownField {
            instruction: instruction.result(),
            field,
        });
        return;
    };
    verify_field_owner(instruction, base, field, declared, values, errors);
    verify_aggregate_result(instruction, declared.field_type, errors);
}

fn verify_field_set(
    instruction: &MirInstruction,
    operands: FieldSetOperands,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declared) = schema.fields.get(&operands.field) else {
        errors.push(MirVerificationError::UnknownField {
            instruction: instruction.result(),
            field: operands.field,
        });
        return;
    };
    verify_field_owner(
        instruction,
        operands.base,
        operands.field,
        declared,
        values,
        errors,
    );
    if !declared.mutable {
        errors.push(MirVerificationError::ImmutableFieldSet {
            instruction: instruction.result(),
            field: operands.field,
        });
    }
    verify_operand_type(
        instruction.result(),
        operands.value,
        declared.field_type,
        values,
        errors,
    );
    if arena.source_type("nil") != Some(instruction.result_type()) {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_field_owner(
    instruction: &MirInstruction,
    base: ValueId,
    field: FieldId,
    declared: &DeclaredField,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    if let Some(found) = values.get(&base)
        && !declared.owner_types.contains(found)
    {
        errors.push(MirVerificationError::WrongFieldOwner {
            instruction: instruction.result(),
            field,
            expected: declared
                .owner_types
                .iter()
                .next()
                .copied()
                .unwrap_or(*found),
            found: *found,
        });
    }
}

fn verify_union_make(
    instruction: &MirInstruction,
    union: SymbolId,
    case: UnionCaseId,
    arguments: &[ValueId],
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declaration) = schema.unions.get(&union) else {
        errors.push(MirVerificationError::UnknownUnion {
            instruction: instruction.result(),
            union,
        });
        return;
    };
    verify_aggregate_result(instruction, declaration.type_id, errors);
    let Some(case_definition) = declaration
        .cases
        .iter()
        .find(|candidate| candidate.case == case)
    else {
        errors.push(MirVerificationError::UnknownUnionCase {
            instruction: instruction.result(),
            union,
            case,
        });
        return;
    };
    for (argument, expected) in arguments.iter().zip(&case_definition.parameters) {
        verify_operand_type(instruction.result(), *argument, *expected, values, errors);
    }
    if arguments.len() != case_definition.parameters.len() {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_callable_instruction(
    instruction: &MirInstruction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    signatures: CallableSignatures<'_>,
    errors: &mut Vec<MirVerificationError>,
) -> bool {
    match instruction.kind() {
        MirInstructionKind::FunctionReference(function) => {
            if let Some((parameters, results, _)) = signatures.functions.get(function)
                && arena.get(instruction.result_type())
                    != Some(&SemanticType::Function {
                        is_async: signatures.async_functions.contains(function),
                        parameters: parameters.clone(),
                        results: results.clone(),
                        effects: pop_types::EffectSummary::empty(),
                    })
            {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::TaskCreate {
            dispatch,
            arguments,
            completion_type,
            ..
        } => {
            let signature = match dispatch {
                MirTaskDispatch::Direct(function) => signatures
                    .async_functions
                    .contains(function)
                    .then(|| signatures.functions.get(function))
                    .flatten(),
                MirTaskDispatch::Referenced(function) => signatures
                    .async_references
                    .contains(function)
                    .then(|| signatures.references.get(function))
                    .flatten(),
                MirTaskDispatch::Indirect(callee) => {
                    let Some(callee_type) = values.get(callee).copied() else {
                        return true;
                    };
                    let Some(SemanticType::Function {
                        is_async: true,
                        parameters,
                        results,
                        ..
                    }) = arena.get(callee_type)
                    else {
                        errors.push(MirVerificationError::InvalidCallableOperand {
                            instruction: instruction.result(),
                            operand: *callee,
                            found: callee_type,
                        });
                        return true;
                    };
                    verify_task_signature(
                        instruction,
                        arguments,
                        parameters,
                        results,
                        *completion_type,
                        arena,
                        values,
                        errors,
                    );
                    return true;
                }
            };
            if let Some((parameters, results, _)) = signature {
                verify_task_signature(
                    instruction,
                    arguments,
                    parameters,
                    results,
                    *completion_type,
                    arena,
                    values,
                    errors,
                );
            } else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::CallDirect {
            function,
            arguments,
            ..
        } => {
            if let Some((parameters, results, _)) = signatures.functions.get(function) {
                verify_call_signature(instruction, arguments, parameters, results, values, errors);
            }
        }
        MirInstructionKind::CallReferenced {
            function,
            arguments,
            ..
        } => {
            if let Some((parameters, results, _)) = signatures.references.get(function) {
                verify_call_signature(instruction, arguments, parameters, results, values, errors);
            }
        }
        MirInstructionKind::CallStandard {
            function,
            arguments,
            ..
        } => {
            let parameter = match function.raw() {
                0 => arena.source_type("Int"),
                1 => arena.source_type("String"),
                _ => {
                    errors.push(MirVerificationError::UnknownStandardFunction(*function));
                    None
                }
            };
            if let Some(parameter) = parameter {
                verify_call_signature(instruction, arguments, &[parameter], &[], values, errors);
            }
        }
        MirInstructionKind::CallDirectMethod {
            method, arguments, ..
        } => {
            if let Some((parameters, results, _)) = signatures.methods.get(method) {
                verify_call_signature(instruction, arguments, parameters, results, values, errors);
            }
        }
        MirInstructionKind::CallInterface {
            interface,
            method,
            slot,
            arguments,
            ..
        } => {
            let Some(declaration) = schema.interfaces.get(interface) else {
                errors.push(MirVerificationError::InvalidCallSignature {
                    instruction: instruction.result(),
                    expected_arguments: 0,
                    found_arguments: arguments.len(),
                    expected_results: 0,
                    found_results: usize::from(instruction.has_result()),
                });
                return true;
            };
            let Some(required) = declaration
                .methods()
                .iter()
                .find(|candidate| candidate.method() == *method && candidate.slot() == *slot)
            else {
                errors.push(MirVerificationError::InvalidCallSignature {
                    instruction: instruction.result(),
                    expected_arguments: declaration.methods().len(),
                    found_arguments: arguments.len(),
                    expected_results: 0,
                    found_results: usize::from(instruction.has_result()),
                });
                return true;
            };
            let receiver_type = arguments
                .first()
                .and_then(|receiver| values.get(receiver))
                .copied();
            let receiver_valid = receiver_type.is_some_and(|receiver_type| {
                receiver_type == declaration.type_id()
                    || schema.classes.values().any(|class| {
                        class.type_id() == receiver_type
                            && class.interfaces().iter().any(|implementation| {
                                implementation.interface() == *interface
                                    && implementation.interface_type() == declaration.type_id()
                            })
                    })
            });
            if !receiver_valid {
                errors.push(MirVerificationError::InvalidCallSignature {
                    instruction: instruction.result(),
                    expected_arguments: required.parameters().len() + 1,
                    found_arguments: arguments.len(),
                    expected_results: required.results().len(),
                    found_results: usize::from(instruction.has_result()),
                });
                return true;
            }
            let mut parameters = vec![receiver_type.expect("validated receiver type")];
            parameters.extend_from_slice(required.parameters());
            verify_call_signature(
                instruction,
                arguments,
                &parameters,
                required.results(),
                values,
                errors,
            );
        }
        MirInstructionKind::CallIndirect {
            callee, arguments, ..
        } => {
            verify_indirect_call(instruction, *callee, arguments, arena, values, errors);
        }
        _ => return false,
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn verify_task_signature(
    instruction: &MirInstruction,
    arguments: &[ValueId],
    parameters: &[TypeId],
    results: &[TypeId],
    completion_type: TypeId,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    for (argument, expected) in arguments.iter().zip(parameters) {
        verify_operand_type(instruction.result(), *argument, *expected, values, errors);
    }
    let expected_completion = match results {
        [result] => Some(*result),
        results => arena.find(&SemanticType::Tuple(results.to_vec())),
    };
    let task_definition = embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.type_by_source_name("Task").copied())
        .map(|entry| entry.id());
    let valid_result = matches!(
        (task_definition, arena.get(instruction.result_type())),
        (
            Some(expected),
            Some(SemanticType::Builtin { definition, arguments })
        ) if *definition == expected && arguments.as_slice() == [completion_type]
    );
    if arguments.len() != parameters.len()
        || expected_completion != Some(completion_type)
        || !valid_result
    {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_indirect_call(
    instruction: &MirInstruction,
    callee: ValueId,
    arguments: &[ValueId],
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(callee_type) = values.get(&callee).copied() else {
        return;
    };
    let Some(SemanticType::Function {
        parameters,
        results,
        ..
    }) = arena.get(callee_type).cloned()
    else {
        errors.push(MirVerificationError::InvalidCallableOperand {
            instruction: instruction.result(),
            operand: callee,
            found: callee_type,
        });
        return;
    };
    verify_call_signature(
        instruction,
        arguments,
        &parameters,
        &results,
        values,
        errors,
    );
}

fn verify_call_signature(
    instruction: &MirInstruction,
    arguments: &[ValueId],
    parameters: &[TypeId],
    results: &[TypeId],
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    for (argument, expected) in arguments.iter().zip(parameters) {
        verify_operand_type(instruction.result(), *argument, *expected, values, errors);
    }
    let found_results = usize::from(instruction.has_result());
    if arguments.len() != parameters.len() || results.len() != found_results {
        errors.push(MirVerificationError::InvalidCallSignature {
            instruction: instruction.result(),
            expected_arguments: parameters.len(),
            found_arguments: arguments.len(),
            expected_results: results.len(),
            found_results,
        });
        return;
    }
    if let ([expected], Some(found)) = (results, instruction.optional_result_type())
        && *expected != found
    {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: found,
        });
    }
}

fn is_optional_of(arena: &TypeArena, candidate: TypeId, element: TypeId) -> bool {
    let Some(nil) = arena.source_type("nil") else {
        return false;
    };
    if element == nil {
        return candidate == nil;
    }
    matches!(
        arena.get(candidate),
        Some(SemanticType::Union(members))
            if members.len() == 2 && members.contains(&element) && members.contains(&nil)
    )
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

fn verify_operand_type(
    instruction: ValueId,
    operand: ValueId,
    expected: TypeId,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    if let Some(found) = values.get(&operand)
        && *found != expected
    {
        errors.push(MirVerificationError::WrongOperandType {
            instruction,
            operand,
            expected,
            found: *found,
        });
    }
}

fn verify_value_use(
    operand: ValueId,
    use_block: BlockId,
    use_instruction: usize,
    facts: &ControlFlowFacts<'_, '_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(definition) = facts.definitions.get(&operand).copied() else {
        errors.push(MirVerificationError::UnknownValue(operand));
        return;
    };
    if definition.block == use_block {
        if definition
            .instruction
            .is_some_and(|definition| definition >= use_instruction)
        {
            errors.push(MirVerificationError::ValueUsedBeforeDefinition(operand));
        }
        return;
    }
    if !facts
        .dominators
        .get(&use_block)
        .is_some_and(|blocks| blocks.contains(&definition.block))
    {
        errors.push(MirVerificationError::ValueNotDominated {
            value: operand,
            definition: definition.block,
            use_block,
        });
    }
}

fn verify_terminator(
    block: &MirBlock,
    function: &MirFunction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    facts: &ControlFlowFacts<'_, '_>,
    expected_suspend_frames: &BTreeMap<BlockId, Vec<MirFrameSlot>>,
    errors: &mut Vec<MirVerificationError>,
) {
    let use_instruction = block.instructions.len();
    match &block.terminator {
        MirTerminator::Missing => errors.push(MirVerificationError::MissingTerminator(block.block)),
        MirTerminator::Branch { target, arguments } => {
            verify_target(*target, facts.blocks, errors);
            for argument in arguments {
                verify_value_use(*argument, block.block, use_instruction, facts, errors);
            }
            verify_edge_arguments(
                block.block,
                *target,
                arguments,
                facts.values,
                facts.blocks,
                errors,
            );
        }
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => {
            verify_value_use(*condition, block.block, use_instruction, facts, errors);
            if let Some(found) = facts.values.get(condition)
                && arena.source_type("Boolean") != Some(*found)
            {
                errors.push(MirVerificationError::ConditionalBranchConditionType {
                    block: block.block,
                    found: *found,
                });
            }
            for target in [*when_true, *when_false] {
                verify_target(target, facts.blocks, errors);
                verify_edge_arguments(block.block, target, &[], facts.values, facts.blocks, errors);
            }
        }
        MirTerminator::UnionSwitch {
            scrutinee,
            union,
            arms,
        } => {
            verify_value_use(*scrutinee, block.block, use_instruction, facts, errors);
            let Some(declaration) = schema.unions.get(union) else {
                errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
                return;
            };
            if facts.values.get(scrutinee) != Some(&declaration.type_id()) {
                errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
            }
            let expected: BTreeSet<_> =
                declaration.cases().iter().map(MirUnionCase::case).collect();
            let found: BTreeSet<_> = arms.iter().map(|arm| arm.case).collect();
            if expected != found || found.len() != arms.len() {
                errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
            }
            for arm in arms {
                verify_target(arm.target, facts.blocks, errors);
                let Some(case) = declaration
                    .cases()
                    .iter()
                    .find(|case| case.case == arm.case)
                else {
                    continue;
                };
                let Some(target) = facts.blocks.get(&arm.target) else {
                    continue;
                };
                if target.arguments.len() != case.parameters.len()
                    || target
                        .arguments
                        .iter()
                        .map(|argument| argument.type_id)
                        .ne(case.parameters.iter().copied())
                {
                    errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
                }
            }
        }
        MirTerminator::ErrorSwitch {
            scrutinee,
            error,
            arms,
        } => {
            verify_value_use(*scrutinee, block.block, use_instruction, facts, errors);
            let Some(declaration) = schema.errors.get(error) else {
                errors.push(MirVerificationError::InvalidErrorSwitch { error: *error });
                return;
            };
            if !matches!(
                facts.values.get(scrutinee).and_then(|type_id| arena.get(*type_id)),
                Some(SemanticType::ErrorUnion { definition, .. }) if *definition == *error
            ) {
                errors.push(MirVerificationError::InvalidErrorSwitch { error: *error });
            }
            let expected: BTreeSet<_> =
                declaration.cases().iter().map(MirErrorCase::case).collect();
            let found: BTreeSet<_> = arms.iter().map(|arm| arm.case).collect();
            if expected != found || found.len() != arms.len() {
                errors.push(MirVerificationError::InvalidErrorSwitch { error: *error });
            }
            for arm in arms {
                verify_target(arm.target, facts.blocks, errors);
                let Some(case) = declaration
                    .cases()
                    .iter()
                    .find(|case| case.case == arm.case)
                else {
                    continue;
                };
                let Some(target) = facts.blocks.get(&arm.target) else {
                    continue;
                };
                if target.arguments.len() != case.parameters.len()
                    || target
                        .arguments
                        .iter()
                        .map(|argument| argument.type_id)
                        .ne(case.parameters.iter().copied())
                {
                    errors.push(MirVerificationError::InvalidErrorSwitch { error: *error });
                }
            }
        }
        MirTerminator::Return { values: returned } => {
            if returned.len() != function.results.len() {
                errors.push(MirVerificationError::WrongReturnArity {
                    expected: function.results.len(),
                    found: returned.len(),
                });
            }
            for (value, expected) in returned.iter().zip(&function.results) {
                verify_value_use(*value, block.block, use_instruction, facts, errors);
                match facts.values.get(value) {
                    Some(found) if found != expected => {
                        errors.push(MirVerificationError::WrongReturnType {
                            expected: *expected,
                            found: *found,
                        });
                    }
                    None => errors.push(MirVerificationError::UnknownValue(*value)),
                    _ => {}
                }
            }
        }
        MirTerminator::Suspend {
            operation: MirSuspendOperation::Task { task, result_type },
            resume,
            cancellation,
            unwind,
            safe_point,
            live_frame,
        } => {
            if !function.is_async {
                errors.push(MirVerificationError::SuspendOutsideAsync(block.block));
            }
            verify_value_use(*task, block.block, use_instruction, facts, errors);
            let task_definition = embedded_bootstrap_schema()
                .ok()
                .and_then(|schema| schema.type_by_source_name("Task").copied())
                .map(|entry| entry.id());
            let valid_task = matches!(
                (task_definition, facts.values.get(task).and_then(|type_id| arena.get(*type_id))),
                (
                    Some(expected),
                    Some(SemanticType::Builtin { definition, arguments })
                ) if *definition == expected && arguments.as_slice() == [*result_type]
            );
            if !valid_task {
                errors.push(MirVerificationError::InvalidSuspendTask(block.block));
            }

            verify_target(*resume, facts.blocks, errors);
            if !facts.blocks.get(resume).is_some_and(|target| {
                target.arguments.len() == 1 && target.arguments[0].type_id == *result_type
            }) {
                errors.push(MirVerificationError::InvalidSuspendResume(block.block));
            }
            verify_target(*cancellation, facts.blocks, errors);
            if !facts.blocks.get(cancellation).is_some_and(|target| {
                target.arguments.is_empty()
                    && target
                        .cleanup
                        .is_some_and(|cleanup| cleanup.reason == MirCleanupExitReason::Cancellation)
            }) {
                errors.push(MirVerificationError::InvalidSuspendCancellation(
                    block.block,
                ));
            }
            if let MirUnwindAction::Cleanup(target) = unwind {
                verify_target(*target, facts.blocks, errors);
                if !facts.blocks.get(target).is_some_and(|target| {
                    target.arguments.is_empty()
                        && target
                            .cleanup
                            .is_some_and(|cleanup| cleanup.reason == MirCleanupExitReason::Unwind)
                }) {
                    errors.push(MirVerificationError::InvalidSuspendFrame(block.block));
                }
            }

            let mut values = BTreeSet::new();
            let expected_roots = live_frame
                .slots
                .iter()
                .enumerate()
                .filter_map(|(index, slot)| {
                    verify_value_use(slot.value, block.block, use_instruction, facts, errors);
                    if facts.values.get(&slot.value) != Some(&slot.type_id)
                        || !values.insert(slot.value)
                    {
                        errors.push(MirVerificationError::InvalidSuspendFrame(block.block));
                    }
                    is_managed_reference_type_id(slot.type_id, Some(arena))
                        .then(|| RootSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
                })
                .collect::<Vec<_>>();
            if live_frame.state.raw()
                >= function
                    .blocks
                    .iter()
                    .filter(|candidate| {
                        matches!(candidate.terminator, MirTerminator::Suspend { .. })
                    })
                    .count() as u32
                || live_frame.stack_map.safe_point() != *safe_point
                || live_frame.stack_map.root_slots() != expected_roots
                || expected_suspend_frames.get(&block.block) != Some(&live_frame.slots)
                || !values.contains(task)
            {
                errors.push(MirVerificationError::InvalidSuspendFrame(block.block));
            }
        }
        MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::ResumeUnwind
        | MirTerminator::Unreachable => {}
    }
}

fn verify_target(
    target: BlockId,
    blocks: &BTreeMap<BlockId, &MirBlock>,
    errors: &mut Vec<MirVerificationError>,
) {
    if !blocks.contains_key(&target) {
        errors.push(MirVerificationError::InvalidBlock(target));
    }
}

fn verify_edge_arguments(
    block: BlockId,
    target: BlockId,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(target_block) = blocks.get(&target) else {
        return;
    };
    if arguments.len() != target_block.arguments.len() {
        errors.push(MirVerificationError::EdgeArgumentArity {
            block,
            target,
            expected: target_block.arguments.len(),
            found: arguments.len(),
        });
    }
    for (index, (argument, parameter)) in arguments.iter().zip(&target_block.arguments).enumerate()
    {
        if let Some(found) = values.get(argument)
            && *found != parameter.type_id
        {
            errors.push(MirVerificationError::EdgeArgumentType {
                block,
                target,
                index,
                expected: parameter.type_id,
                found: *found,
            });
        }
    }
}

pub(crate) fn instruction_operands(kind: &MirInstructionKind) -> Vec<ValueId> {
    match kind {
        MirInstructionKind::IntegerConstant(_)
        | MirInstructionKind::FloatConstant(_)
        | MirInstructionKind::StringConstant(_)
        | MirInstructionKind::BooleanConstant(_)
        | MirInstructionKind::NilConstant
        | MirInstructionKind::EnumConstant { .. }
        | MirInstructionKind::FunctionReference(_)
        | MirInstructionKind::GcSafePoint { .. } => Vec::new(),
        MirInstructionKind::TupleMake(values)
        | MirInstructionKind::ArrayMake {
            elements: values, ..
        }
        | MirInstructionKind::CallDirect {
            arguments: values, ..
        }
        | MirInstructionKind::CallReferenced {
            arguments: values, ..
        }
        | MirInstructionKind::CallStandard {
            arguments: values, ..
        }
        | MirInstructionKind::CallDirectMethod {
            arguments: values, ..
        }
        | MirInstructionKind::CallInterface {
            arguments: values, ..
        }
        | MirInstructionKind::CallBuiltinInterface {
            arguments: values, ..
        }
        | MirInstructionKind::UnionMake {
            arguments: values, ..
        }
        | MirInstructionKind::ResultMake {
            arguments: values, ..
        }
        | MirInstructionKind::IterationMake {
            arguments: values, ..
        }
        | MirInstructionKind::ErrorMake {
            arguments: values, ..
        } => values.clone(),
        MirInstructionKind::TaskCreate {
            dispatch,
            arguments,
            ..
        } => match dispatch {
            MirTaskDispatch::Direct(_) | MirTaskDispatch::Referenced(_) => arguments.clone(),
            MirTaskDispatch::Indirect(callee) => std::iter::once(*callee)
                .chain(arguments.iter().copied())
                .collect(),
        },
        MirInstructionKind::TupleGet { tuple, .. } => vec![*tuple],
        MirInstructionKind::IterationIsItem { iteration, .. }
        | MirInstructionKind::IterationGetItem { iteration, .. } => vec![*iteration],
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            ..
        } => vec![*length, *initial_value],
        MirInstructionKind::ListCreate { capacity, .. } => capacity.iter().copied().collect(),
        MirInstructionKind::RangeCreate { first, last, step } => vec![*first, *last, *step],
        MirInstructionKind::CallIndirect {
            callee, arguments, ..
        } => std::iter::once(*callee)
            .chain(arguments.iter().copied())
            .collect(),
        MirInstructionKind::CheckedIntegerAdd { left, right, .. }
        | MirInstructionKind::CheckedIntegerSubtract { left, right, .. }
        | MirInstructionKind::CheckedIntegerMultiply { left, right, .. }
        | MirInstructionKind::CheckedIntegerDivide { left, right, .. }
        | MirInstructionKind::CheckedIntegerRemainder { left, right, .. }
        | MirInstructionKind::FloatAdd { left, right, .. }
        | MirInstructionKind::FloatSubtract { left, right, .. }
        | MirInstructionKind::FloatMultiply { left, right, .. }
        | MirInstructionKind::FloatDivide { left, right, .. }
        | MirInstructionKind::BooleanAnd { left, right }
        | MirInstructionKind::BooleanOr { left, right }
        | MirInstructionKind::CompareEqual { left, right }
        | MirInstructionKind::CompareNotEqual { left, right }
        | MirInstructionKind::CompareIntegerLess { left, right, .. }
        | MirInstructionKind::CompareIntegerLessOrEqual { left, right, .. }
        | MirInstructionKind::CompareIntegerGreater { left, right, .. }
        | MirInstructionKind::CompareIntegerGreaterOrEqual { left, right, .. }
        | MirInstructionKind::CompareFloatLess { left, right, .. }
        | MirInstructionKind::CompareFloatLessOrEqual { left, right, .. }
        | MirInstructionKind::CompareFloatGreater { left, right, .. }
        | MirInstructionKind::CompareFloatGreaterOrEqual { left, right, .. }
        | MirInstructionKind::StringConcat { left, right } => vec![*left, *right],
        MirInstructionKind::BooleanNot { operand }
        | MirInstructionKind::OptionalIsPresent { optional: operand }
        | MirInstructionKind::OptionalGet { optional: operand }
        | MirInstructionKind::IntegerNegate { operand, .. }
        | MirInstructionKind::FloatNegate { operand, .. }
        | MirInstructionKind::ConvertInteger { operand, .. }
        | MirInstructionKind::ConvertIntegerToFloat { operand, .. }
        | MirInstructionKind::ConvertFloatToInteger { operand, .. }
        | MirInstructionKind::ConvertFloat { operand, .. }
        | MirInstructionKind::StringFormat { value: operand, .. } => vec![*operand],
        MirInstructionKind::ResultIsOk { result, .. }
        | MirInstructionKind::ResultGetOk { result, .. }
        | MirInstructionKind::ResultGetError { result, .. } => vec![*result],
        MirInstructionKind::ArrayGet { array, index } => vec![*array, *index],
        MirInstructionKind::ListGet { list, index }
        | MirInstructionKind::ListGetChecked { list, index } => vec![*list, *index],
        MirInstructionKind::TableGet { table, key } => vec![*table, *key],
        MirInstructionKind::ArrayLength { array } => vec![*array],
        MirInstructionKind::ListLength { list } => vec![*list],
        MirInstructionKind::ArrayGetChecked { array, index } => vec![*array, *index],
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            ..
        } => vec![*array, *index, *value],
        MirInstructionKind::ArrayFill { array, value, .. } => vec![*array, *value],
        MirInstructionKind::ListSet {
            list, index, value, ..
        } => vec![*list, *index, *value],
        MirInstructionKind::ListAdd { list, value, .. } => vec![*list, *value],
        MirInstructionKind::TableSet {
            table, key, value, ..
        } => vec![*table, *key, *value],
        MirInstructionKind::RecordMake { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstructionKind::ClassMake { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstructionKind::TableMake { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [*key, *value])
            .collect(),
        MirInstructionKind::RecordUpdate { base, fields, .. } => std::iter::once(*base)
            .chain(fields.iter().map(|(_, value)| *value))
            .collect(),
        MirInstructionKind::FieldGet { base, .. } => vec![*base],
        MirInstructionKind::InterfaceUpcast { value: base, .. }
        | MirInstructionKind::CaptureCellLoad { cell: base } => vec![*base],
        MirInstructionKind::CaptureCellAllocate { initial, .. } => vec![*initial],
        MirInstructionKind::CaptureCellStore { cell, value } => vec![*cell, *value],
        MirInstructionKind::ClosureEnvironmentAllocate { captures, .. } => captures
            .iter()
            .filter(|capture| !capture.self_reference)
            .map(|capture| capture.value)
            .collect(),
        MirInstructionKind::CaptureStore { value, .. } => vec![*value],
        MirInstructionKind::CaptureLoad { .. }
        | MirInstructionKind::CaptureCellReference { .. } => Vec::new(),
        MirInstructionKind::FieldSet { base, value, .. } => vec![*base, *value],
        MirInstructionKind::RetainRoot { value } => vec![*value],
        MirInstructionKind::ReleaseRoot { handle } => vec![*handle],
        MirInstructionKind::Pin { value } => vec![*value],
        MirInstructionKind::Unpin { handle } => vec![*handle],
        MirInstructionKind::WriteBarrier {
            owner,
            previous,
            value,
            ..
        } => std::iter::once(*owner)
            .chain(previous.iter().copied())
            .chain(value.iter().copied())
            .collect(),
    }
}
