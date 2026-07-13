//! HIR to canonical MIR lowering and portable effect/GC preparation.
//!
//! This module makes evaluation order, control flow, calls, failure edges,
//! roots, barriers, and safe points explicit. It consumes typed HIR and does
//! not perform source lookup or introduce backend-specific instructions.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    BindingId, BlockId, CaptureId, ClassId, CleanupScopeId, FieldId, FileId, FunctionId, LocalId,
    MethodId, ResultCaseId, SourceSpan, SymbolId, SymbolIdentity, TextRange, TextSize, TypeId,
    ValueId, ValueParameterId,
};
use pop_hir::{
    HirAssignmentTarget, HirBubble, HirCallDispatch, HirCaptureMode, HirCaptureSource, HirClosure,
    HirDataSpecialization, HirDeclaration, HirDeclarationKind, HirErrorMatchArm, HirExpression,
    HirExpressionKind, HirFieldValue, HirFunction, HirMatchArm, HirResultMatchArm, HirStatement,
    HirStatementKind, HirTableEntry, hir_generic_call_instances, specialize_hir_function,
};
use pop_runtime_interface::{
    ArrayElementMap, ObjectMap, ObjectSlot, RootSlot, SafePointId, StackMap, Trap, TrapKind,
};
use pop_types::{
    FloatKind, IntegerKind, IntegerValue, NumericConversionKind, PrimitiveType, SemanticType,
    TypeArena, TypedBinaryOperator, TypedCompoundOperator, TypedUnaryOperator,
};

use crate::ir::*;
use crate::verification::{
    instruction_operands, instruction_unwind_target, terminator_operands, terminator_targets,
    verify_mir_bubble,
};

/// Lowers a verified HIR Bubble to canonical MIR and verifies the result.
///
/// # Errors
///
/// Returns deterministic MIR invariant violations.
pub fn lower_hir_bubble(
    hir: &HirBubble,
    arena: &TypeArena,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    let function_references: Vec<_> = hir
        .function_references()
        .iter()
        .map(|reference| MirFunctionReference {
            identity: reference.identity(),
            parameters: reference.parameters().to_vec(),
            results: reference.results().to_vec(),
            effects: lower_effect_summary(reference.effects()),
        })
        .collect();
    let reference_effects: BTreeMap<_, _> = function_references
        .iter()
        .map(|reference| (reference.identity, reference.effects))
        .collect();
    let declarations: Vec<_> = hir
        .declarations()
        .iter()
        .filter(|declaration| match declaration.kind() {
            HirDeclarationKind::Record(record) => !arena.contains_type_parameter(record.type_id()),
            HirDeclarationKind::Union(union) => !arena.contains_type_parameter(union.type_id()),
            _ => true,
        })
        .filter_map(lower_declaration)
        .collect();
    let gc_schema = LoweringGcSchema::new(&declarations, arena);
    let specialized_hir_functions = specialize_reachable_functions(hir, arena)?;
    let empty_function_effects = BTreeMap::new();
    let empty_method_effects = BTreeMap::new();
    let mut provisional_functions: Vec<_> = specialized_hir_functions
        .iter()
        .map(|function| {
            lower_function(
                function,
                arena,
                &gc_schema,
                &reference_effects,
                &empty_function_effects,
                &empty_method_effects,
            )
            .0
        })
        .collect();
    let mut provisional_methods: Vec<_> = hir
        .methods()
        .iter()
        .map(|method| MirMethod {
            method: method.method(),
            class: method.class(),
            function: lower_function(
                method.function(),
                arena,
                &gc_schema,
                &reference_effects,
                &empty_function_effects,
                &empty_method_effects,
            )
            .0,
        })
        .collect();
    recompute_callable_effects(&mut provisional_functions, &mut provisional_methods);
    let function_effects: BTreeMap<_, _> = provisional_functions
        .iter()
        .map(|function| (function.symbol(), function.effects()))
        .collect();
    let method_effects: BTreeMap<_, _> = provisional_methods
        .iter()
        .map(|method| (method.method(), method.function().effects()))
        .collect();
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
            );
            nested_functions.append(&mut nested);
            function
        })
        .collect();
    functions.sort_by_key(MirFunction::symbol);
    let methods = hir
        .methods()
        .iter()
        .map(|method| {
            let (function, mut nested) = lower_function(
                method.function(),
                arena,
                &gc_schema,
                &reference_effects,
                &function_effects,
                &method_effects,
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
    let mut mir = MirBubble {
        bubble: hir.bubble(),
        namespace: hir.namespace(),
        dependencies: hir.dependencies().to_vec(),
        declarations,
        functions,
        methods,
        nested_functions,
        function_references,
    };
    recompute_effects(&mut mir);
    while insert_gc_safe_points(&mut mir, arena) {
        // Backedge safe points make their containing function a GC safe point.
        // Recompute the transitive call effects before deciding which callers
        // also require a safe point immediately before the call.
        recompute_effects(&mut mir);
    }
    seal_effects(&mut mir);
    verify_mir_bubble(&mir, arena)?;
    Ok(mir)
}

fn specialize_reachable_functions(
    hir: &HirBubble,
    arena: &TypeArena,
) -> Result<Vec<HirFunction>, Vec<MirVerificationError>> {
    let templates: BTreeMap<_, _> = hir
        .functions()
        .iter()
        .map(|function| (function.symbol(), function))
        .collect();
    let mut instances = BTreeMap::new();
    let data_symbols: BTreeMap<_, _> = hir
        .declarations()
        .iter()
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
    for template in hir.declarations().iter().filter(|declaration| {
        matches!(declaration.kind(), HirDeclarationKind::Record(record)
            if arena.contains_type_parameter(record.type_id()))
    }) {
        let HirDeclarationKind::Record(template_record) = template.kind() else {
            continue;
        };
        for instance in hir.declarations().iter().filter(|declaration| {
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
    let data_instances = HirDataSpecialization::new(data_symbols, data_fields);
    let mut pending = BTreeSet::new();
    for function in hir
        .functions()
        .iter()
        .filter(|function| function.type_parameters().is_empty())
    {
        pending.extend(hir_generic_call_instances(function));
    }
    let mut next_symbol = hir
        .functions()
        .iter()
        .map(HirFunction::symbol)
        .chain(hir.declarations().iter().map(HirDeclaration::symbol))
        .chain(
            hir.methods()
                .iter()
                .map(|method| method.function().symbol()),
        )
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
    for function in hir
        .functions()
        .iter()
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
            class: class.class(),
            type_id: class.type_id(),
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
) -> (MirFunction, Vec<MirNestedFunction>) {
    let (mut lowered, nested) = FunctionBuilder::new(
        function,
        arena,
        gc_schema,
        reference_effects,
        function_effects,
        method_effects,
    )
    .lower();
    lowered.function = function.function();
    (lowered, nested)
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
    body: &'hir [HirStatement],
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
    Table {
        table: ValueId,
        key: ValueId,
        table_type: TypeId,
        value_type: TypeId,
    },
}

struct FunctionBuilder<'hir> {
    owner: SymbolId,
    parameters_schema: Vec<TypeId>,
    results: Vec<TypeId>,
    body: &'hir [HirStatement],
    capture_schema: BTreeMap<CaptureId, MirCapture>,
    arena: &'hir TypeArena,
    gc_schema: &'hir LoweringGcSchema,
    reference_effects: &'hir BTreeMap<SymbolIdentity, MirEffectSummary>,
    function_effects: &'hir BTreeMap<SymbolId, MirEffectSummary>,
    method_effects: &'hir BTreeMap<MethodId, MirEffectSummary>,
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
        HirStatementKind::Defer { body } => {
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
        HirStatementKind::Defer { body } => contains_continue_for_current_loop(body),
        HirStatementKind::While { .. }
        | HirStatementKind::OptionalWhile { .. }
        | HirStatementKind::RepeatUntil { .. }
        | HirStatementKind::NumericFor { .. }
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
        HirExpressionKind::Field { base, .. }
        | HirExpressionKind::TupleGet { tuple: base, .. }
        | HirExpressionKind::InterfaceUpcast { value: base, .. }
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
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_) => {}
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
    ) -> Self {
        let parameter_specs: Vec<_> = hir
            .parameters()
            .iter()
            .map(|parameter| (parameter.parameter(), parameter.type_id(), parameter.span()))
            .collect();
        Self::from_parts(
            hir.symbol(),
            parameter_specs,
            hir.results().to_vec(),
            hir.body(),
            BTreeMap::new(),
            arena,
            gc_schema,
            reference_effects,
            function_effects,
            method_effects,
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
            parameter_specs,
            closure.results().to_vec(),
            closure.body(),
            capture_schema,
            arena,
            gc_schema,
            reference_effects,
            function_effects,
            method_effects,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        owner: SymbolId,
        parameter_specs: Vec<(ValueParameterId, TypeId, SourceSpan)>,
        results: Vec<TypeId>,
        body: &'hir [HirStatement],
        capture_schema: BTreeMap<CaptureId, MirCapture>,
        arena: &'hir TypeArena,
        gc_schema: &'hir LoweringGcSchema,
        reference_effects: &'hir BTreeMap<SymbolIdentity, MirEffectSummary>,
        function_effects: &'hir BTreeMap<SymbolId, MirEffectSummary>,
        method_effects: &'hir BTreeMap<MethodId, MirEffectSummary>,
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
        let function = MirFunction {
            function: FunctionId::from_raw(0),
            symbol: self.owner,
            parameters: self.parameters_schema,
            results: self.results,
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
                HirStatementKind::Defer { body } => {
                    let scope = CleanupScopeId::from_raw(self.next_cleanup_scope);
                    self.next_cleanup_scope = self.next_cleanup_scope.saturating_add(1);
                    self.active_cleanups.push(ActiveCleanup { scope, body });
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
                    self.lower_expression(expression);
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
            self.lower_statements(cleanup.body);
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
            HirExpressionKind::Closure(closure) => {
                return self.lower_closure(closure, expression.type_id());
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
                arguments,
                ..
            } => self.lower_call(dispatch, arguments),
            HirExpressionKind::InterfaceUpcast { value, interface } => {
                let value = self.lower_expression(value);
                MirInstructionKind::InterfaceUpcast {
                    value,
                    interface: *interface,
                }
            }
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

    fn lower_closure(&mut self, closure: &HirClosure, closure_type: TypeId) -> ValueId {
        let (lowered, mut nested) = FunctionBuilder::new_closure(
            self.owner,
            closure,
            self.arena,
            self.gc_schema,
            self.reference_effects,
            self.function_effects,
            self.method_effects,
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
        let object_map = closure_environment_object_map(self.arena, &captures);
        self.nested_functions.push(MirNestedFunction {
            owner: self.owner,
            function: closure.function(),
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
        });
        self.nested_functions.append(&mut nested);
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
            } => MirInstructionKind::CallInterface {
                interface: *interface,
                method: *method,
                slot: *slot,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: conservative_indirect_effects(),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::Indirect { callee } => {
                let callee = self.lower_expression(callee);
                MirInstructionKind::CallIndirect {
                    callee,
                    arguments: arguments
                        .iter()
                        .map(|argument| self.lower_expression(argument))
                        .collect(),
                    declared_effects: conservative_indirect_effects(),
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
            self.lower_statements(cleanup.body);
            self.current_cleanup = previous_cleanup;
        }
        self.terminate(MirTerminator::ResumeUnwind);
        self.current = call_block;
        self.active_cleanups = registered;
        let cleanup = cleanup_entry.expect("active cleanup set is nonempty");
        let unwind = match &mut instruction {
            MirInstructionKind::CallDirect { unwind, .. }
            | MirInstructionKind::CallReferenced { unwind, .. }
            | MirInstructionKind::CallDirectMethod { unwind, .. }
            | MirInstructionKind::CallInterface { unwind, .. }
            | MirInstructionKind::CallIndirect { unwind, .. } => Some(unwind),
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

pub(crate) fn is_managed_reference_type_id(type_id: TypeId, arena: Option<&TypeArena>) -> bool {
    let Some(arena) = arena else {
        return false;
    };
    matches!(
        arena.get(type_id),
        Some(
            SemanticType::Primitive(PrimitiveType::String)
                | SemanticType::Tuple(_)
                | SemanticType::Array(_)
                | SemanticType::Table { .. }
                | SemanticType::Class { .. }
                | SemanticType::Interface { .. }
                | SemanticType::Builtin { .. }
                | SemanticType::ErrorUnion { .. }
        )
    )
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
        | MirInstructionKind::ArrayGetChecked { .. } => {
            MirEffectSummary::empty().with(MirEffect::MayTrap)
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
        | MirInstructionKind::ArrayFill { element_map, .. } => {
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
        MirInstructionKind::TupleMake(_)
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
        MirInstructionKind::GcSafePoint { .. } => {
            MirEffectSummary::empty().with(MirEffect::GcSafePoint)
        }
        MirInstructionKind::RetainRoot { .. }
        | MirInstructionKind::ReleaseRoot { .. }
        | MirInstructionKind::Pin { .. }
        | MirInstructionKind::Unpin { .. } => MirEffectSummary::empty().with(MirEffect::Roots),
        MirInstructionKind::WriteBarrier { .. } => {
            MirEffectSummary::empty().with(MirEffect::WritesManagedReference)
        }
        MirInstructionKind::CaptureCellStore { .. } | MirInstructionKind::CaptureStore { .. } => {
            MirEffectSummary::from_effects([MirEffect::WritesManagedReference])
        }
        MirInstructionKind::CallDirect {
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
        | MirInstructionKind::CallIndirect {
            declared_effects, ..
        } => *declared_effects,
        MirInstructionKind::IntegerConstant(_)
        | MirInstructionKind::FloatConstant(_)
        | MirInstructionKind::StringConstant(_)
        | MirInstructionKind::BooleanConstant(_)
        | MirInstructionKind::NilConstant
        | MirInstructionKind::OptionalIsPresent { .. }
        | MirInstructionKind::OptionalGet { .. }
        | MirInstructionKind::ResultMake { .. }
        | MirInstructionKind::ErrorMake { .. }
        | MirInstructionKind::ResultIsOk { .. }
        | MirInstructionKind::ResultGetOk { .. }
        | MirInstructionKind::ResultGetError { .. }
        | MirInstructionKind::EnumConstant { .. }
        | MirInstructionKind::FunctionReference(_)
        | MirInstructionKind::TupleGet { .. }
        | MirInstructionKind::ArrayGet { .. }
        | MirInstructionKind::TableGet { .. }
        | MirInstructionKind::ArrayLength { .. }
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
        | MirInstructionKind::FieldGet { .. }
        | MirInstructionKind::FieldSet { .. }
        | MirInstructionKind::UnionMake { .. } => MirEffectSummary::empty(),
        MirInstructionKind::InterfaceUpcast { .. }
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
        MirTerminator::Missing
        | MirTerminator::Branch { .. }
        | MirTerminator::ConditionalBranch { .. }
        | MirTerminator::UnionSwitch { .. }
        | MirTerminator::ErrorSwitch { .. }
        | MirTerminator::Return { .. }
        | MirTerminator::Unreachable => MirEffectSummary::empty(),
    }
}

fn conservative_indirect_effects() -> MirEffectSummary {
    MirEffectSummary::from_effects([
        MirEffect::Allocates,
        MirEffect::WritesManagedReference,
        MirEffect::MayTrap,
        MirEffect::MayUnwind,
        MirEffect::Suspends,
        MirEffect::UnsafeMemory,
        MirEffect::ForeignFunction,
        MirEffect::AmbientIo,
        MirEffect::GcSafePoint,
        MirEffect::Roots,
    ])
}

fn recompute_effects(bubble: &mut MirBubble) {
    recompute_callable_effects(&mut bubble.functions, &mut bubble.methods);
}

fn recompute_callable_effects(functions: &mut [MirFunction], methods: &mut [MirMethod]) {
    let mut function_effects: BTreeMap<_, _> = functions
        .iter()
        .map(|function| (function.symbol, function.effects))
        .collect();
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

fn insert_gc_safe_points(bubble: &mut MirBubble, arena: &TypeArena) -> bool {
    let mut changed = false;
    for function in &mut bubble.functions {
        changed |= insert_function_safe_points(function, arena);
    }
    for method in &mut bubble.methods {
        changed |= insert_function_safe_points(&mut method.function, arena);
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
        .flat_map(|block| block.instructions.iter())
        .filter_map(|instruction| match instruction.kind {
            MirInstructionKind::GcSafePoint { safe_point, .. } => Some(safe_point.raw()),
            _ => None,
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
                        | MirInstructionKind::CallDirectMethod { .. }
                        | MirInstructionKind::CallInterface { .. }
                        | MirInstructionKind::CallIndirect { .. }
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

pub(crate) fn expected_safe_point_roots(
    function: &MirFunction,
    arena: &TypeArena,
) -> BTreeMap<ValueId, Vec<ValueId>> {
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

    let mut maps = BTreeMap::new();
    for block in &function.blocks {
        let mut live = live_out.get(&block.block).cloned().unwrap_or_default();
        live.extend(terminator_operands(&block.terminator));
        for instruction in block.instructions.iter().rev() {
            if let MirInstructionKind::GcSafePoint { .. } = instruction.kind {
                let roots = live
                    .iter()
                    .copied()
                    .filter(|value| {
                        value_types.get(value).is_some_and(|type_id| {
                            is_managed_reference_type_id(*type_id, Some(arena))
                        })
                    })
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
            .flat_map(|arm| live_in.get(&arm.target).into_iter().flatten().copied())
            .collect(),
        MirTerminator::ErrorSwitch { arms, .. } => arms
            .iter()
            .flat_map(|arm| live_in.get(&arm.target).into_iter().flatten().copied())
            .collect(),
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
    for (parameter, argument) in target.arguments().iter().zip(arguments) {
        if live.remove(&parameter.value()) {
            live.insert(*argument);
        }
    }
    live
}
