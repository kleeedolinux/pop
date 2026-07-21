//! Bubble-wide LLVM backend analysis, declarations, and dispatch helpers.
//!
//! These routines derive backend-private tables from verified MIR. They do
//! not recover source semantics or mutate canonical MIR.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use pop_foundation::{BubbleId, ClassId, FieldId, SymbolId, TypeId, ValueId};
use pop_mir::{MirBubble, MirDeclarationKind, MirEffect, MirEffectSummary, MirInstructionKind};
use pop_runtime_interface::RuntimeOperation;
use pop_types::{SemanticType, TypeArena};

use crate::api::LlvmLoweringError;
use crate::instruction_lowering::{
    llvm_results, llvm_type, lower_builtin_iteration_call, nested_function_tag,
};
use crate::lowering::{
    PrivateBlock, PrivateFunction, async_function_create_name, async_indirect_create_name,
    async_nested_create_name, builtin_interface_name, checked_downcast_name,
    direct_scalar_array_fill_name, function_name, indirect_name, interface_name,
    llvm_memory_none_instruction, method_name, native_runtime_symbol, nested_name,
};

pub(crate) fn direct_scalar_array_fill_function(bubble: BubbleId) -> PrivateFunction {
    PrivateFunction {
        name: direct_scalar_array_fill_name(bubble),
        parameters: vec![
            "ptr %storage".to_owned(),
            "i64 %length".to_owned(),
            "i64 %value".to_owned(),
        ],
        result: "void".to_owned(),
        blocks: vec![
            PrivateBlock {
                label: "entry".to_owned(),
                instructions: vec!["%empty = icmp eq i64 %length, 0".to_owned()],
                terminator: "br i1 %empty, label %done, label %fill".to_owned(),
            },
            PrivateBlock {
                label: "fill".to_owned(),
                instructions: vec![
                    "%index = phi i64 [ 0, %entry ], [ %next, %fill ]".to_owned(),
                    "%slot = getelementptr i64, ptr %storage, i64 %index".to_owned(),
                    "store i64 %value, ptr %slot, align 8".to_owned(),
                    "%next = add nuw i64 %index, 1".to_owned(),
                    "%filled = icmp eq i64 %next, %length".to_owned(),
                ],
                terminator: "br i1 %filled, label %done, label %fill".to_owned(),
            },
            PrivateBlock {
                label: "done".to_owned(),
                instructions: Vec::new(),
                terminator: "ret void".to_owned(),
            },
        ],
        attributes: vec!["nounwind"],
        internal: false,
    }
}

pub(crate) fn checked_integer_declarations() -> Vec<String> {
    let mut declarations = [8_u16, 16, 32, 64]
        .into_iter()
        .flat_map(|bits| {
            ["sadd", "uadd", "ssub", "usub", "smul", "umul"].map(move |operation| {
                format!(
                    "declare {{ i{bits}, i1 }} @llvm.{operation}.with.overflow.i{bits}(i{bits}, i{bits})"
                )
            })
        })
        .collect::<Vec<_>>();
    declarations.extend([
        "declare float @llvm.trunc.f32(float)".to_owned(),
        "declare double @llvm.trunc.f64(double)".to_owned(),
    ]);
    declarations
}

pub(crate) type ClassRuntimeKeys = BTreeMap<(ClassId, TypeId), String>;

pub(crate) fn lower_checked_downcast_helpers(
    bubble: &MirBubble,
    class_runtime_keys: &ClassRuntimeKeys,
) -> Vec<PrivateFunction> {
    let targets = bubble
        .functions()
        .iter()
        .flat_map(pop_mir::MirFunction::blocks)
        .chain(
            bubble
                .methods()
                .iter()
                .flat_map(|method| method.function().blocks()),
        )
        .chain(
            bubble
                .nested_functions()
                .iter()
                .flat_map(pop_mir::MirNestedFunction::blocks),
        )
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::CheckedDowncast {
                target_class,
                target_type,
                ..
            } => Some((*target_class, *target_type)),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let classes = bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Class(class) => Some((class.class(), class)),
            _ => None,
        })
        .chain(
            bubble
                .nominal_references()
                .classes()
                .iter()
                .map(|reference| (reference.class(), reference.declaration())),
        )
        .collect::<BTreeMap<_, _>>();
    targets
        .into_iter()
        .map(|(target, target_type)| {
            let accepted = class_runtime_keys
                .iter()
                .filter(|((class, type_id), _)| {
                    class_specialization_descends_from(
                        bubble,
                        &classes,
                        *class,
                        *type_id,
                        target,
                        target_type,
                    )
                })
                .map(|(_, key)| key)
                .collect::<Vec<_>>();
            let mut instructions = vec![format!(
                "%class = call i64 @{}(i64 %v0, i64 1)",
                native_runtime_symbol(RuntimeOperation::FieldGet)
            )];
            let mut matched = "false".to_owned();
            for (index, class) in accepted.iter().enumerate() {
                let comparison = format!("%match_{index}");
                instructions.push(format!("{comparison} = icmp eq i64 %class, {class}",));
                if index == 0 {
                    matched = comparison;
                } else {
                    let combined = format!("%matched_{index}");
                    instructions.push(format!("{combined} = or i1 {matched}, {comparison}"));
                    matched = combined;
                }
            }
            PrivateFunction {
                name: checked_downcast_name(bubble.bubble(), target, target_type),
                parameters: vec!["i64 %v0".to_owned()],
                result: "i1".to_owned(),
                blocks: vec![PrivateBlock {
                    label: "entry".to_owned(),
                    instructions,
                    terminator: format!("ret i1 {matched}"),
                }],
                attributes: vec!["nounwind"],
                internal: true,
            }
        })
        .collect()
}

pub(crate) fn collect_class_runtime_keys(
    bubble: &MirBubble,
    types: &pop_types::TypeArena,
) -> ClassRuntimeKeys {
    (0..types.len())
        .filter_map(|raw| {
            let type_id = TypeId::from_raw(u32::try_from(raw).ok()?);
            let pop_types::SemanticType::Class { class, .. } = types.get(type_id)? else {
                return None;
            };
            let identity = bubble.canonical_class_identity(types, *class, type_id)?;
            Some(((*class, type_id), nominal_runtime_key(&identity)))
        })
        .collect()
}

pub(crate) fn render_class_runtime_descriptors(
    class_runtime_keys: &ClassRuntimeKeys,
) -> Vec<String> {
    class_runtime_keys
        .values()
        .map(|key| {
            let symbol = key
                .strip_prefix("ptrtoint (ptr @")
                .and_then(|key| key.strip_suffix(" to i64)"))
                .expect("runtime key uses one exact descriptor symbol");
            format!("@{symbol} = linkonce_odr constant i8 0")
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn nominal_runtime_key(identity: &pop_types::CanonicalNominalIdentity) -> String {
    let encoded = identity
        .descriptor()
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("ptrtoint (ptr @pop_nominal_{encoded} to i64)")
}

fn class_descends_from(
    classes: &BTreeMap<ClassId, &pop_mir::MirClassDeclaration>,
    concrete: ClassId,
    target: ClassId,
) -> bool {
    let mut current = concrete;
    let mut visited = BTreeSet::new();
    while visited.insert(current) {
        if current == target {
            return true;
        }
        let Some(base) = classes.get(&current).and_then(|class| class.base()) else {
            return false;
        };
        current = base;
    }
    false
}

fn class_specialization_descends_from(
    bubble: &MirBubble,
    classes: &BTreeMap<ClassId, &pop_mir::MirClassDeclaration>,
    concrete: ClassId,
    concrete_type: TypeId,
    target: ClassId,
    target_type: TypeId,
) -> bool {
    if concrete == target {
        return concrete_type == target_type;
    }
    if let Some(reference) = bubble
        .nominal_references()
        .classes()
        .iter()
        .find(|candidate| candidate.class() == concrete && candidate.type_id() == concrete_type)
    {
        let mut current = reference.base().zip(reference.base_type());
        let mut visited = BTreeSet::new();
        while let Some((class, type_id)) = current {
            if !visited.insert((class, type_id)) {
                return false;
            }
            if class == target {
                return type_id == target_type;
            }
            current = bubble
                .nominal_references()
                .classes()
                .iter()
                .find(|candidate| candidate.class() == class && candidate.type_id() == type_id)
                .and_then(|candidate| candidate.base().zip(candidate.base_type()));
        }
        return false;
    }
    class_descends_from(classes, concrete, target)
}

pub(crate) fn collect_string_literals(bubble: &MirBubble) -> BTreeMap<String, String> {
    let values = bubble
        .functions()
        .iter()
        .chain(bubble.methods().iter().map(pop_mir::MirMethod::function))
        .flat_map(pop_mir::MirFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::StringConstant(value) => Some(value.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let nested_values = bubble
        .nested_functions()
        .iter()
        .flat_map(pop_mir::MirNestedFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::StringConstant(value) => Some(value.clone()),
            _ => None,
        });
    let codec_labels = bubble
        .generated_codec_adapters()
        .iter()
        .flat_map(|adapter| {
            adapter
                .members()
                .iter()
                .map(|member| member.name().to_owned())
        });
    let values = values
        .into_iter()
        .chain(nested_values)
        .chain(codec_labels)
        .collect::<BTreeSet<_>>();
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| (value, format!("@pop_string_{index}")))
        .collect()
}

pub(crate) fn analyze_memory_none_functions(bubble: &MirBubble) -> BTreeSet<SymbolId> {
    let mut candidates = bubble
        .functions()
        .iter()
        .filter(|function| {
            function
                .effects()
                .is_subset_of(MirEffectSummary::empty().with(MirEffect::MayTrap))
                && function
                    .blocks()
                    .iter()
                    .flat_map(pop_mir::MirBlock::instructions)
                    .all(|instruction| {
                        matches!(instruction.kind(), MirInstructionKind::CallDirect { .. })
                            || llvm_memory_none_instruction(instruction.kind())
                    })
        })
        .map(pop_mir::MirFunction::symbol)
        .collect::<BTreeSet<_>>();
    loop {
        let rejected = bubble
            .functions()
            .iter()
            .filter(|function| candidates.contains(&function.symbol()))
            .filter(|function| {
                function
                    .blocks()
                    .iter()
                    .flat_map(pop_mir::MirBlock::instructions)
                    .any(|instruction| {
                        matches!(
                            instruction.kind(),
                            MirInstructionKind::CallDirect { function, .. }
                                if !candidates.contains(function)
                        )
                    })
            })
            .map(pop_mir::MirFunction::symbol)
            .collect::<Vec<_>>();
        if rejected.is_empty() {
            return candidates;
        }
        for function in rejected {
            candidates.remove(&function);
        }
    }
}

pub(crate) fn collect_self_capture_slots(
    bubble: &MirBubble,
) -> BTreeMap<(SymbolId, pop_foundation::NestedFunctionId), BTreeSet<u32>> {
    let mut slots = BTreeMap::new();
    for instruction in bubble
        .functions()
        .iter()
        .map(pop_mir::MirFunction::blocks)
        .chain(
            bubble
                .methods()
                .iter()
                .map(|method| method.function().blocks()),
        )
        .chain(
            bubble
                .nested_functions()
                .iter()
                .map(pop_mir::MirNestedFunction::blocks),
        )
        .flatten()
        .flat_map(pop_mir::MirBlock::instructions)
    {
        if let MirInstructionKind::ClosureEnvironmentAllocate {
            owner,
            function,
            captures,
            ..
        } = instruction.kind()
        {
            slots
                .entry((*owner, *function))
                .or_insert_with(BTreeSet::new)
                .extend(
                    captures
                        .iter()
                        .filter(|capture| capture.self_reference())
                        .map(|capture| capture.slot()),
                );
        }
    }
    slots
}

pub(crate) fn render_string_literals(literals: &BTreeMap<String, String>) -> Vec<String> {
    literals
        .iter()
        .map(|(value, symbol)| {
            let bytes = value
                .as_bytes()
                .iter()
                .fold(String::new(), |mut output, byte| {
                    let _ = write!(output, "\\{byte:02X}");
                    output
                });
            format!(
                "{symbol} = private unnamed_addr constant [{} x i8] c\"{bytes}\"",
                value.len()
            )
        })
        .collect()
}

pub(crate) fn runtime_declarations() -> Vec<String> {
    vec![
        "declare { i64, i64 } @pop_rt_bytes_view_lengths(i64) nounwind".to_owned(),
        "declare { i64, i64 } @pop_rt_text_view_lengths(i64) nounwind".to_owned(),
        "declare { i1, i64, i64, i64 } @pop_rt_bytes_view_slice(i64, i64, i64, i64, i64, i64) nounwind".to_owned(),
        "declare { i1, i64, i64, i64 } @pop_rt_text_view_slice(i64, i64, i64, i64, i64, i64) nounwind".to_owned(),
        "declare { i1, i8 } @pop_rt_bytes_view_get(i64, i64, i64, i64) nounwind".to_owned(),
        "declare i64 @pop_rt_bytes_view_materialize(i64, i64, i64) nounwind".to_owned(),
        "declare i64 @pop_rt_text_view_materialize(i64, i64, i64) nounwind".to_owned(),
        format!(
            "declare i8 @{}(i64, i64, i1, ptr) nounwind",
            pop_runtime_native_abi::TABLE_GET_CHECKED_SYMBOL
        ),
        format!(
            "declare i8 @{}(i64, i64, i64, i1, i1) nounwind",
            native_runtime_symbol(RuntimeOperation::TableSet)
        ),
        format!(
            "declare i8 @{}(i64, ptr) nounwind",
            native_runtime_symbol(RuntimeOperation::ArrayLength)
        ),
        format!(
            "declare i8 @{}(i64, i64, ptr) nounwind",
            native_runtime_symbol(RuntimeOperation::ArrayGetChecked)
        ),
        format!(
            "declare i64 @{}(i64, i64) nounwind",
            native_runtime_symbol(RuntimeOperation::FieldGet)
        ),
        format!(
            "declare i8 @{}(i64, i64, i64) nounwind",
            native_runtime_symbol(RuntimeOperation::ArraySet)
        ),
        format!(
            "declare i8 @{}(i64, i64) nounwind",
            native_runtime_symbol(RuntimeOperation::ArrayFill)
        ),
        format!(
            "declare i64 @{}(i64, i1) nounwind",
            native_runtime_symbol(RuntimeOperation::ListCreate)
        ),
        format!(
            "declare i8 @{}(i64, ptr) nounwind",
            native_runtime_symbol(RuntimeOperation::ListLength)
        ),
        format!(
            "declare i8 @{}(i64, i64, ptr) nounwind",
            native_runtime_symbol(RuntimeOperation::ListGet)
        ),
        format!(
            "declare i8 @{}(i64, i64, ptr) nounwind",
            native_runtime_symbol(RuntimeOperation::ListGetChecked)
        ),
        format!(
            "declare i8 @{}(i64, i64, i64, i1) nounwind",
            native_runtime_symbol(RuntimeOperation::ListSet)
        ),
        format!(
            "declare i8 @{}(i64, i64, i1) nounwind",
            native_runtime_symbol(RuntimeOperation::ListAdd)
        ),
        format!(
            "declare i64 @{}(i64, i64, i64, i1, i8) nounwind",
            native_runtime_symbol(RuntimeOperation::RangeCreate)
        ),
        format!(
            "declare i64 @{}(i64, i8) nounwind",
            native_runtime_symbol(RuntimeOperation::IterationAcquire)
        ),
        format!(
            "declare i8 @{}(i64, ptr) nounwind",
            native_runtime_symbol(RuntimeOperation::IterationNext)
        ),
        format!(
            "declare i8 @{}(i64, i64, i64) nounwind",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
    ]
}

pub(crate) fn collect_field_layout(bubble: &MirBubble) -> BTreeMap<FieldId, u32> {
    let mut layout = BTreeMap::new();
    for declaration in bubble.declarations() {
        let (fields, reserved_slots) = match declaration.kind() {
            MirDeclarationKind::Record(record) => (record.fields(), 0_u32),
            MirDeclarationKind::Class(class) => (class.fields(), 1_u32),
            MirDeclarationKind::Union(_)
            | MirDeclarationKind::Error(_)
            | MirDeclarationKind::Enum(_)
            | MirDeclarationKind::Interface(_) => continue,
        };
        for (slot, field) in fields.iter().enumerate() {
            if let Ok(slot) = u32::try_from(slot) {
                layout.insert(field.field(), slot + reserved_slots + 1);
            }
        }
    }
    layout
}

pub(crate) fn collect_record_fields(bubble: &MirBubble) -> BTreeMap<SymbolId, Vec<FieldId>> {
    bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Record(record) => Some((
                declaration.symbol(),
                record
                    .fields()
                    .iter()
                    .map(pop_mir::MirField::field)
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

pub(crate) fn collect_record_field_types(bubble: &MirBubble) -> BTreeMap<TypeId, Vec<TypeId>> {
    bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Record(record) => Some((
                record.type_id(),
                record
                    .fields()
                    .iter()
                    .map(pop_mir::MirField::field_type)
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

pub(crate) fn lower_interface_dispatchers(
    bubble: &MirBubble,
    types: &TypeArena,
    class_runtime_keys: &ClassRuntimeKeys,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    let classes = bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Class(class) => Some(class),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut dispatchers = Vec::new();
    for interface in
        bubble
            .declarations()
            .iter()
            .filter_map(|declaration| match declaration.kind() {
                MirDeclarationKind::Interface(interface) => Some(interface),
                _ => None,
            })
    {
        for method in interface.methods() {
            let implementations = classes
                .iter()
                .filter_map(|class| {
                    class
                        .interfaces()
                        .iter()
                        .find(|implementation| implementation.interface() == interface.interface())
                        .and_then(|implementation| {
                            implementation.methods().iter().find(|implementation| {
                                implementation.interface_method() == method.method()
                            })
                        })
                        .map(|implementation| {
                            (
                                class.class(),
                                class.type_id(),
                                implementation.class_method(),
                            )
                        })
                })
                .collect::<Vec<_>>();
            dispatchers.push(lower_interface_dispatcher(
                bubble.bubble(),
                interface.interface(),
                method,
                &implementations,
                types,
                class_runtime_keys,
            )?);
        }
    }
    Ok(dispatchers)
}

pub(crate) fn lower_builtin_interface_dispatchers(
    bubble: &MirBubble,
    types: &TypeArena,
    class_runtime_keys: &ClassRuntimeKeys,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    let protocol = pop_types::embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol())
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let mut calls = BTreeSet::new();
    for blocks in bubble
        .functions()
        .iter()
        .map(pop_mir::MirFunction::blocks)
        .chain(
            bubble
                .methods()
                .iter()
                .map(|method| method.function().blocks()),
        )
    {
        let value_types = collect_block_value_types(blocks);
        for instruction in blocks.iter().flat_map(pop_mir::MirBlock::instructions) {
            let MirInstructionKind::CallBuiltinInterface {
                method, arguments, ..
            } = instruction.kind()
            else {
                continue;
            };
            let Some(receiver) = arguments.first().and_then(|value| value_types.get(value)) else {
                continue;
            };
            if matches!(
                types.get(*receiver),
                Some(SemanticType::Builtin { definition, .. })
                    if *definition == protocol.iterator()
            ) {
                calls.insert((*receiver, *method, instruction.result_type()));
            }
        }
    }
    calls
        .into_iter()
        .map(|(receiver, method, result)| {
            lower_builtin_interface_dispatcher(
                bubble,
                receiver,
                method,
                result,
                types,
                class_runtime_keys,
            )
        })
        .collect()
}

fn lower_builtin_interface_dispatcher(
    bubble: &MirBubble,
    receiver: TypeId,
    method: pop_foundation::IterationProtocolMethodId,
    result: TypeId,
    types: &TypeArena,
    class_runtime_keys: &ClassRuntimeKeys,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let Some(SemanticType::Builtin { definition, .. }) = types.get(receiver) else {
        return Err(LlvmLoweringError::InvalidType(receiver));
    };
    let implementations = bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Class(class) => class
                .builtin_interfaces()
                .iter()
                .find(|implementation| {
                    implementation.interface() == *definition
                        && implementation.interface_type() == receiver
                })
                .and_then(|implementation| {
                    implementation
                        .methods()
                        .iter()
                        .find(|implementation| implementation.protocol_method() == method)
                })
                .map(|implementation| {
                    (
                        class.class(),
                        class.type_id(),
                        implementation.class_method(),
                    )
                }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let first_check = if implementations.is_empty() {
        "native".to_owned()
    } else {
        "class_check_0".to_owned()
    };
    let mut blocks = vec![PrivateBlock {
        label: "dispatch".to_owned(),
        instructions: vec![format!(
            "%dispatch_tag = call i64 @{}(i64 %v0, i64 1)",
            native_runtime_symbol(RuntimeOperation::FieldGet)
        )],
        terminator: format!("br label %{first_check}"),
    }];
    for (index, (class, class_type, _)) in implementations.iter().enumerate() {
        let key = class_runtime_keys
            .get(&(*class, *class_type))
            .ok_or(LlvmLoweringError::InvalidType(*class_type))?;
        let otherwise = if index + 1 == implementations.len() {
            "native".to_owned()
        } else {
            format!("class_check_{}", index + 1)
        };
        blocks.push(PrivateBlock {
            label: format!("class_check_{index}"),
            instructions: vec![format!(
                "%class_match_{index} = icmp eq i64 %dispatch_tag, {key}"
            )],
            terminator: format!(
                "br i1 %class_match_{index}, label %class_{}_t{}, label %{otherwise}",
                class.raw(),
                class_type.raw()
            ),
        });
    }
    for (class, class_type, class_method) in implementations {
        blocks.push(PrivateBlock {
            label: format!("class_{}_t{}", class.raw(), class_type.raw()),
            instructions: vec![format!(
                "%class_result_{} = call i64 @{}(i64 %v0)",
                class.raw(),
                method_name(bubble.bubble(), class_method)
            )],
            terminator: format!("ret i64 %class_result_{}", class.raw()),
        });
    }
    let values = BTreeMap::from([(ValueId::from_raw(0), receiver)]);
    blocks.push(PrivateBlock {
        label: "native".to_owned(),
        instructions: vec![lower_builtin_iteration_call(
            "%native_result",
            result,
            method,
            &[ValueId::from_raw(0)],
            &values,
            types,
        )?],
        terminator: "ret i64 %native_result".to_owned(),
    });
    Ok(PrivateFunction {
        name: builtin_interface_name(bubble.bubble(), receiver, method),
        parameters: vec!["i64 %v0".to_owned()],
        result: "i64".to_owned(),
        blocks,
        attributes: Vec::new(),
        internal: false,
    })
}

pub(crate) fn lower_interface_dispatcher(
    bubble: BubbleId,
    interface: pop_foundation::InterfaceId,
    method: &pop_mir::MirInterfaceMethod,
    implementations: &[(ClassId, TypeId, pop_foundation::MethodId)],
    types: &TypeArena,
    class_runtime_keys: &ClassRuntimeKeys,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let result_type = llvm_results(method.results(), types)?;
    let mut parameters = vec!["i64 %v0".to_owned()];
    parameters.extend(
        method
            .parameters()
            .iter()
            .enumerate()
            .map(|(index, type_id)| {
                llvm_type(*type_id, types).map(|ty| format!("{ty} %v{}", index + 1))
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let first_check = if implementations.is_empty() {
        "invalid_dispatch".to_owned()
    } else {
        "class_check_0".to_owned()
    };
    let mut blocks = vec![PrivateBlock {
        label: "dispatch".to_owned(),
        instructions: vec![format!(
            "%dispatch_tag = call i64 @{}(i64 %v0, i64 1)",
            native_runtime_symbol(RuntimeOperation::FieldGet)
        )],
        terminator: format!("br label %{first_check}"),
    }];
    for (index, (class, class_type, _)) in implementations.iter().enumerate() {
        let key = class_runtime_keys
            .get(&(*class, *class_type))
            .ok_or(LlvmLoweringError::InvalidType(*class_type))?;
        let otherwise = if index + 1 == implementations.len() {
            "invalid_dispatch".to_owned()
        } else {
            format!("class_check_{}", index + 1)
        };
        blocks.push(PrivateBlock {
            label: format!("class_check_{index}"),
            instructions: vec![format!(
                "%class_match_{index} = icmp eq i64 %dispatch_tag, {key}"
            )],
            terminator: format!(
                "br i1 %class_match_{index}, label %class_{}_t{}, label %{otherwise}",
                class.raw(),
                class_type.raw()
            ),
        });
    }
    let arguments = std::iter::once("i64 %v0".to_owned())
        .chain(
            method
                .parameters()
                .iter()
                .enumerate()
                .map(|(index, type_id)| {
                    llvm_type(*type_id, types).map(|ty| format!("{ty} %v{}", index + 1))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .collect::<Vec<_>>()
        .join(", ");
    for (class, class_type, class_method) in implementations {
        let dispatch_result = format!("%dispatch_result_{}_t{}", class.raw(), class_type.raw());
        let (instructions, terminator) = if method.results().is_empty() {
            (
                vec![format!(
                    "call void @{}({arguments})",
                    method_name(bubble, *class_method)
                )],
                "ret void".to_owned(),
            )
        } else {
            (
                vec![format!(
                    "{dispatch_result} = call {result_type} @{}({arguments})",
                    method_name(bubble, *class_method)
                )],
                format!("ret {result_type} {dispatch_result}"),
            )
        };
        blocks.push(PrivateBlock {
            label: format!("class_{}_t{}", class.raw(), class_type.raw()),
            instructions,
            terminator,
        });
    }
    blocks.push(PrivateBlock {
        label: "invalid_dispatch".to_owned(),
        instructions: Vec::new(),
        terminator: format!(
            "call void @{}()\n  unreachable",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
    });
    Ok(PrivateFunction {
        name: interface_name(bubble, interface, method.method()),
        parameters,
        result: result_type,
        blocks,
        attributes: Vec::new(),
        internal: false,
    })
}

pub(crate) fn lower_indirect_dispatchers(
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    let mut function_types = BTreeSet::new();
    for blocks in bubble
        .functions()
        .iter()
        .map(pop_mir::MirFunction::blocks)
        .chain(
            bubble
                .methods()
                .iter()
                .map(|method| method.function().blocks()),
        )
        .chain(
            bubble
                .nested_functions()
                .iter()
                .map(pop_mir::MirNestedFunction::blocks),
        )
    {
        let value_types = collect_block_value_types(blocks);
        for instruction in blocks.iter().flat_map(pop_mir::MirBlock::instructions) {
            if let MirInstructionKind::CallIndirect { callee, .. } = instruction.kind()
                && let Some(type_id) = value_types.get(callee)
            {
                function_types.insert(*type_id);
            }
        }
    }
    function_types
        .into_iter()
        .map(|type_id| lower_indirect_dispatcher(type_id, bubble, types))
        .collect()
}

pub(crate) fn lower_async_indirect_create_dispatchers(
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    let mut function_types = BTreeSet::new();
    for blocks in bubble
        .functions()
        .iter()
        .map(pop_mir::MirFunction::blocks)
        .chain(
            bubble
                .methods()
                .iter()
                .map(|method| method.function().blocks()),
        )
        .chain(
            bubble
                .nested_functions()
                .iter()
                .map(pop_mir::MirNestedFunction::blocks),
        )
    {
        let value_types = collect_block_value_types(blocks);
        for instruction in blocks.iter().flat_map(pop_mir::MirBlock::instructions) {
            match instruction.kind() {
                MirInstructionKind::TaskCreate {
                    dispatch: pop_mir::MirTaskDispatch::Indirect(callee),
                    ..
                } => {
                    if let Some(type_id) = value_types.get(callee) {
                        function_types.insert(*type_id);
                    }
                }
                MirInstructionKind::TaskGroupCreate { body, .. } => {
                    if let Some(type_id) = value_types.get(body) {
                        function_types.insert(*type_id);
                    }
                }
                _ => {}
            }
        }
    }
    function_types
        .into_iter()
        .map(|type_id| lower_async_indirect_create_dispatcher(type_id, bubble, types))
        .collect()
}

fn lower_async_indirect_create_dispatcher(
    function_type: TypeId,
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let Some(SemanticType::Function {
        is_async: true,
        parameters: parameter_types,
        results: result_types,
        ..
    }) = types.get(function_type)
    else {
        return Err(LlvmLoweringError::InvalidType(function_type));
    };
    let typed_arguments = parameter_types
        .iter()
        .enumerate()
        .map(|(index, type_id)| {
            llvm_type(*type_id, types).map(|ty| format!("{ty} %v{}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut parameters = vec!["i64 %v0".to_owned()];
    parameters.extend(typed_arguments.clone());
    parameters.push("i64 %pop_cancel_token".to_owned());
    let call_arguments = typed_arguments
        .iter()
        .cloned()
        .chain(std::iter::once("i64 %pop_cancel_token".to_owned()))
        .collect::<Vec<_>>()
        .join(", ");
    let direct = bubble
        .functions()
        .iter()
        .filter(|function| {
            function.is_async()
                && function.parameters() == parameter_types
                && function.results() == result_types
        })
        .collect::<Vec<_>>();
    let nested = bubble
        .nested_functions()
        .iter()
        .filter(|function| {
            function.is_async()
                && function.parameters() == parameter_types
                && function.results() == result_types
        })
        .collect::<Vec<_>>();
    let direct_cases = direct
        .iter()
        .map(|function| {
            format!(
                "    i64 {}, label %direct_s{}",
                function.symbol().raw(),
                function.symbol().raw()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let nested_cases = nested
        .iter()
        .map(|function| {
            format!(
                "    i64 {}, label %nested_{}_{}",
                nested_function_tag(function.owner(), function.function()),
                function.owner().raw(),
                function.function().raw()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut blocks = vec![
        PrivateBlock {
            label: "dispatch".to_owned(),
            instructions: vec![
                format!(
                    "%callable_tag = call i64 @{}(i64 %v0, i64 1)",
                    native_runtime_symbol(RuntimeOperation::FieldGet)
                ),
                "%direct_bits = and i64 %callable_tag, 9223372036854775808".to_owned(),
                "%is_direct = icmp ne i64 %direct_bits, 0".to_owned(),
            ],
            terminator: "br i1 %is_direct, label %direct, label %closure".to_owned(),
        },
        PrivateBlock {
            label: "direct".to_owned(),
            instructions: vec![
                "%direct_symbol = and i64 %callable_tag, 9223372036854775807".to_owned(),
            ],
            terminator: format!(
                "switch i64 %direct_symbol, label %invalid_async [\n{direct_cases}\n  ]"
            ),
        },
        PrivateBlock {
            label: "closure".to_owned(),
            instructions: Vec::new(),
            terminator: format!(
                "switch i64 %callable_tag, label %invalid_async [\n{nested_cases}\n  ]"
            ),
        },
    ];
    blocks.extend(direct.into_iter().map(|function| PrivateBlock {
        label: format!("direct_s{}", function.symbol().raw()),
        instructions: vec![format!(
            "%direct_task_{} = call i64 @{}({call_arguments})",
            function.symbol().raw(),
            async_function_create_name(bubble.bubble(), function.symbol())
        )],
        terminator: format!("ret i64 %direct_task_{}", function.symbol().raw()),
    }));
    blocks.extend(nested.into_iter().map(|function| {
        let arguments = std::iter::once("i64 %v0".to_owned())
            .chain(typed_arguments.iter().cloned())
            .chain(std::iter::once("i64 %pop_cancel_token".to_owned()))
            .collect::<Vec<_>>()
            .join(", ");
        PrivateBlock {
            label: format!(
                "nested_{}_{}",
                function.owner().raw(),
                function.function().raw()
            ),
            instructions: vec![format!(
                "%nested_task_{}_{} = call i64 @{}({arguments})",
                function.owner().raw(),
                function.function().raw(),
                async_nested_create_name(bubble.bubble(), function.owner(), function.function())
            )],
            terminator: format!(
                "ret i64 %nested_task_{}_{}",
                function.owner().raw(),
                function.function().raw()
            ),
        }
    }));
    blocks.push(PrivateBlock {
        label: "invalid_async".to_owned(),
        instructions: vec![format!(
            "call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        )],
        terminator: "unreachable".to_owned(),
    });
    Ok(PrivateFunction {
        name: async_indirect_create_name(bubble.bubble(), function_type),
        parameters,
        result: "i64".to_owned(),
        blocks,
        attributes: Vec::new(),
        internal: false,
    })
}

pub(crate) fn collect_block_value_types(blocks: &[pop_mir::MirBlock]) -> BTreeMap<ValueId, TypeId> {
    blocks
        .iter()
        .flat_map(|block| {
            block
                .arguments()
                .iter()
                .map(|argument| (argument.value(), argument.type_id()))
                .chain(block.instructions().iter().filter_map(|instruction| {
                    instruction
                        .optional_result_type()
                        .map(|type_id| (instruction.result(), type_id))
                }))
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
pub(crate) fn lower_indirect_dispatcher(
    function_type: TypeId,
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let Some(SemanticType::Function {
        parameters: parameter_types,
        results: result_types,
        ..
    }) = types.get(function_type)
    else {
        return Err(LlvmLoweringError::InvalidType(function_type));
    };
    let result_type = llvm_results(result_types, types)?;
    let mut parameters = vec!["i64 %v0".to_owned()];
    let typed_arguments = parameter_types
        .iter()
        .enumerate()
        .map(|(index, type_id)| {
            llvm_type(*type_id, types).map(|ty| format!("{ty} %v{}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    parameters.extend(typed_arguments.clone());
    let argument_text = typed_arguments.join(", ");
    let direct = bubble
        .functions()
        .iter()
        .filter(|function| {
            function.parameters() == parameter_types && function.results() == result_types
        })
        .collect::<Vec<_>>();
    let nested = bubble
        .nested_functions()
        .iter()
        .filter(|function| {
            function.parameters() == parameter_types && function.results() == result_types
        })
        .collect::<Vec<_>>();
    let direct_cases = direct
        .iter()
        .map(|function| {
            format!(
                "    i64 {}, label %direct_s{}",
                function.symbol().raw(),
                function.symbol().raw()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let nested_cases = nested
        .iter()
        .map(|function| {
            format!(
                "    i64 {}, label %nested_{}_{}",
                nested_function_tag(function.owner(), function.function()),
                function.owner().raw(),
                function.function().raw()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut blocks = vec![
        PrivateBlock {
            label: "dispatch".to_owned(),
            instructions: vec![
                format!(
                    "%callable_tag = call i64 @{}(i64 %v0, i64 1)",
                    native_runtime_symbol(RuntimeOperation::FieldGet)
                ),
                "%direct_bits = and i64 %callable_tag, 9223372036854775808".to_owned(),
                "%is_direct = icmp ne i64 %direct_bits, 0".to_owned(),
            ],
            terminator: "br i1 %is_direct, label %direct, label %closure".to_owned(),
        },
        PrivateBlock {
            label: "direct".to_owned(),
            instructions: vec![
                "%direct_symbol = and i64 %callable_tag, 9223372036854775807".to_owned(),
            ],
            terminator: format!(
                "switch i64 %direct_symbol, label %invalid_indirect [\n{direct_cases}\n  ]"
            ),
        },
        PrivateBlock {
            label: "closure".to_owned(),
            instructions: Vec::new(),
            terminator: format!(
                "switch i64 %callable_tag, label %invalid_indirect [\n{nested_cases}\n  ]"
            ),
        },
    ];
    for function in direct {
        blocks.push(indirect_call_target(
            format!("direct_s{}", function.symbol().raw()),
            &format!("@{}", function_name(bubble.bubble(), function.symbol())),
            &argument_text,
            &result_type,
            result_types.is_empty(),
        ));
    }
    for function in nested {
        let arguments = if argument_text.is_empty() {
            "i64 %v0".to_owned()
        } else {
            format!("i64 %v0, {argument_text}")
        };
        blocks.push(indirect_call_target(
            format!(
                "nested_{}_{}",
                function.owner().raw(),
                function.function().raw()
            ),
            &format!(
                "@{}",
                nested_name(bubble.bubble(), function.owner(), function.function())
            ),
            &arguments,
            &result_type,
            result_types.is_empty(),
        ));
    }
    blocks.push(PrivateBlock {
        label: "invalid_indirect".to_owned(),
        instructions: Vec::new(),
        terminator: format!(
            "call void @{}()\n  unreachable",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
    });
    Ok(PrivateFunction {
        name: indirect_name(bubble.bubble(), function_type),
        parameters,
        result: result_type,
        blocks,
        attributes: Vec::new(),
        internal: false,
    })
}

pub(crate) fn indirect_call_target(
    label: String,
    callee: &str,
    arguments: &str,
    result_type: &str,
    returns_void: bool,
) -> PrivateBlock {
    if returns_void {
        return PrivateBlock {
            label,
            instructions: vec![format!("call void {callee}({arguments})")],
            terminator: "ret void".to_owned(),
        };
    }
    let result = format!("%indirect_result_{label}");
    PrivateBlock {
        label,
        instructions: vec![format!(
            "{result} = call {result_type} {callee}({arguments})"
        )],
        terminator: format!("ret {result_type} {result}"),
    }
}
