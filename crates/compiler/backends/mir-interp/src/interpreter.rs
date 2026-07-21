//! Verified-MIR execution engine and its public resource-limited API.
//!
//! Construction verifies the complete `MirBubble` before retaining it. Execution
//! consumes resolved stable IDs only and delegates every runtime operation through
//! the backend-neutral PLRI adapter.
use crate::evaluation::*;
use crate::ffi_buffer::{
    integer_from_u64, integer_i64, integer_kind_for_type, integer_u64, marshal, unmarshal,
};
use crate::runtime::ReferenceRuntimeAdapter;
use crate::values::{
    MirClassValue, MirCodecError, MirCodecEvent, MirCodecReader, MirValue, MirViewLenderValue,
    MirViewValue, RuntimeValue,
};
use pop_foundation::{
    BorrowRegionId, ClassId, FfiCallbackSiteId as MirFfiCallbackSiteId, NestedFunctionId, SymbolId,
    SymbolIdentity, TypeId, ValueId,
};
use pop_mir::{
    MirBubble, MirCancellationMode, MirDeclarationKind, MirFfiLayout, MirFfiValueClass,
    MirGeneratedCodecAdapter, MirGeneratedCodecMemberId, MirInstruction, MirInstructionKind,
    MirSuspendOperation, MirTaskDispatch, MirTerminator, MirUnwindAction, MirVerificationError,
    verify_mir_bubble,
};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, BarrierKind, CancellationObservation,
    CancellationTokenId, FfiBufferBorrowId, FfiBufferOpenFailure, FfiBufferOpenRequest,
    FfiBytesBorrowId, FfiCallbackCloseFailure, FfiCallbackLifetime, FfiCallbackOpenFailure,
    FfiCallbackOpenRequest, FfiCallbackRegistration, FfiCallbackRegistrationId, FfiCallbackSiteId,
    FfiCallbackThread, ForeignAddress, ForeignCallMode, ManagedReference, ObjectAllocationRequest,
    ObjectMap, ObjectSlot, PinHandle, RootHandle, RootPublication, RootSlot, RuntimeAdapter,
    RuntimeFailure, RuntimeTypeId, SchedulerId, StackMap, TableAllocationRequest, TaskGroupExit,
    TaskGroupId, TaskGroupLifecycle, TaskId, TaskLifecycle, TaskOwner, TaskPollCompletion,
    TaskState as RuntimeTaskState, Trap, TrapKind, UnwindReason, WriteBarrier,
};
use pop_types::{
    FFI_CALLBACK_CONTEXT_TYPE_ID, FFI_HANDLE_TYPE_ID, FFI_OPTIONAL_POINTER_TYPE_ID,
    FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID, FloatKind, IntegerKind, IntegerValue, PrimitiveType,
    SemanticType, TypeArena, is_ffi_function_type_constructor, is_ffi_integer_abi_builtin_type,
    is_ffi_pointer_type_constructor,
};
use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

const MAX_CODEC_NESTING_DEPTH: u8 = 32;
const MAX_CODEC_EVENTS: usize = 65_536;
const MAX_CODEC_SEQUENCE_ELEMENTS: usize = 65_535;

fn push_codec_event(
    events: &mut Vec<MirCodecEvent>,
    event: MirCodecEvent,
) -> Result<(), MirCodecError> {
    if events.len() >= MAX_CODEC_EVENTS {
        return Err(MirCodecError::LimitExceeded);
    }
    events.push(event);
    Ok(())
}

fn managed_type(arena: &TypeArena, type_id: TypeId) -> bool {
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
                | SemanticType::Function { .. }
                | SemanticType::ErrorUnion { .. }
        )
    )
}

fn ffi_pointer(value: &MirValue) -> Result<ForeignAddress, ExecutionError> {
    let MirValue::FfiPointer(address) = value else {
        return Err(ExecutionError::TypeMismatch);
    };
    Ok(*address)
}

fn encode_codec_value<R: RuntimeAdapter>(
    adapter: &MirGeneratedCodecAdapter,
    value: &MirValue,
    events: &mut Vec<MirCodecEvent>,
    arena: &TypeArena,
    catalog: &[MirGeneratedCodecAdapter],
    runtime: &mut R,
    depth: u8,
) -> Result<(), MirCodecError> {
    if depth > MAX_CODEC_NESTING_DEPTH {
        return Err(MirCodecError::LimitExceeded);
    }
    match value {
        MirValue::Record { record, fields } if *record == adapter.target().symbol() => {
            push_codec_event(
                events,
                MirCodecEvent::RecordStart(
                    u16::try_from(adapter.members().len())
                        .map_err(|_| MirCodecError::LimitExceeded)?,
                ),
            )?;
            for member in adapter.members() {
                let MirGeneratedCodecMemberId::Field(field) = member.member() else {
                    return Err(MirCodecError::CapabilityFailure);
                };
                let field_value = fields
                    .iter()
                    .find_map(|(found, value)| (*found == field).then_some(value))
                    .ok_or(MirCodecError::CapabilityFailure)?;
                push_codec_event(
                    events,
                    MirCodecEvent::Member {
                        ordinal: member.ordinal(),
                        label: member.name().to_owned(),
                    },
                )?;
                encode_codec_scalar(
                    member
                        .types()
                        .first()
                        .copied()
                        .ok_or(MirCodecError::CapabilityFailure)?,
                    field_value,
                    events,
                    arena,
                    catalog,
                    runtime,
                    depth + 1,
                )?;
            }
            push_codec_event(events, MirCodecEvent::RecordEnd)?;
        }
        MirValue::Enum {
            definition,
            case,
            discriminant,
        } if *definition == adapter.target().symbol() => {
            let member = adapter
                .members()
                .iter()
                .find(|member| member.member() == MirGeneratedCodecMemberId::EnumCase(*case))
                .ok_or(MirCodecError::CapabilityFailure)?;
            if member.discriminant() != Some(*discriminant) {
                return Err(MirCodecError::CapabilityFailure);
            }
            push_codec_event(
                events,
                MirCodecEvent::EnumCase {
                    ordinal: member.ordinal(),
                    label: member.name().to_owned(),
                    discriminant: *discriminant,
                },
            )?;
        }
        MirValue::Union {
            union,
            case,
            arguments,
        } if *union == adapter.target().symbol() => {
            let member = adapter
                .members()
                .iter()
                .find(|member| member.member() == MirGeneratedCodecMemberId::UnionCase(*case))
                .ok_or(MirCodecError::CapabilityFailure)?;
            if member.types().len() != arguments.len() {
                return Err(MirCodecError::CapabilityFailure);
            }
            push_codec_event(
                events,
                MirCodecEvent::UnionStart {
                    ordinal: member.ordinal(),
                    label: member.name().to_owned(),
                    payload_count: u16::try_from(arguments.len())
                        .map_err(|_| MirCodecError::LimitExceeded)?,
                },
            )?;
            for (ordinal, (type_id, argument)) in member.types().iter().zip(arguments).enumerate() {
                push_codec_event(
                    events,
                    MirCodecEvent::Payload(
                        u16::try_from(ordinal).map_err(|_| MirCodecError::LimitExceeded)?,
                    ),
                )?;
                encode_codec_scalar(
                    *type_id,
                    argument,
                    events,
                    arena,
                    catalog,
                    runtime,
                    depth + 1,
                )?;
            }
            push_codec_event(events, MirCodecEvent::UnionEnd)?;
        }
        _ => return Err(MirCodecError::CapabilityFailure),
    }
    Ok(())
}

fn encode_codec_scalar<R: RuntimeAdapter>(
    type_id: TypeId,
    value: &MirValue,
    events: &mut Vec<MirCodecEvent>,
    arena: &TypeArena,
    catalog: &[MirGeneratedCodecAdapter],
    runtime: &mut R,
    depth: u8,
) -> Result<(), MirCodecError> {
    if depth > MAX_CODEC_NESTING_DEPTH {
        return Err(MirCodecError::LimitExceeded);
    }
    if let Some(adapter) = catalog
        .iter()
        .find(|adapter| adapter.target_type() == type_id)
    {
        return encode_codec_value(adapter, value, events, arena, catalog, runtime, depth);
    }
    match (arena.get(type_id), value) {
        (Some(SemanticType::Tuple(types)), MirValue::Tuple(values))
            if types.len() == values.len() =>
        {
            push_codec_event(
                events,
                MirCodecEvent::TupleStart(
                    u16::try_from(values.len()).map_err(|_| MirCodecError::LimitExceeded)?,
                ),
            )?;
            for (index, (type_id, value)) in types.iter().zip(values).enumerate() {
                push_codec_event(
                    events,
                    MirCodecEvent::Element(
                        u16::try_from(index).map_err(|_| MirCodecError::LimitExceeded)?,
                    ),
                )?;
                encode_codec_scalar(*type_id, value, events, arena, catalog, runtime, depth + 1)?;
            }
            push_codec_event(events, MirCodecEvent::TupleEnd)?;
            return Ok(());
        }
        (Some(SemanticType::Array(element)), MirValue::Array(values)) => {
            encode_codec_sequence(*element, values, events, arena, catalog, runtime, depth + 1)?;
            return Ok(());
        }
        (
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }),
            MirValue::List(values),
        ) if definition.raw() == 101 && arguments.len() == 1 => {
            encode_codec_sequence(
                arguments[0],
                values,
                events,
                arena,
                catalog,
                runtime,
                depth + 1,
            )?;
            return Ok(());
        }
        (Some(SemanticType::Union(types)), value)
            if types.len() == 2
                && types.iter().any(|type_id| {
                    arena.get(*type_id) == Some(&SemanticType::Primitive(PrimitiveType::Nil))
                }) =>
        {
            if matches!(value, MirValue::Nil) {
                push_codec_event(events, MirCodecEvent::OptionalAbsent)?;
            } else {
                push_codec_event(events, MirCodecEvent::OptionalPresent)?;
                let payload = types
                    .iter()
                    .copied()
                    .find(|type_id| {
                        arena.get(*type_id) != Some(&SemanticType::Primitive(PrimitiveType::Nil))
                    })
                    .ok_or(MirCodecError::CapabilityFailure)?;
                encode_codec_scalar(payload, value, events, arena, catalog, runtime, depth + 1)?;
            }
            return Ok(());
        }
        _ => {}
    }
    let event = match (arena.get(type_id), value) {
        (Some(SemanticType::Primitive(PrimitiveType::Boolean)), MirValue::Boolean(value)) => {
            MirCodecEvent::Boolean(*value)
        }
        (Some(SemanticType::Primitive(PrimitiveType::String)), MirValue::String(value)) => {
            MirCodecEvent::String(value.clone())
        }
        (
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }),
            MirValue::Bytes(reference),
        ) if definition.raw() == 0 && arguments.is_empty() => {
            let length = runtime
                .immutable_bytes_length(*reference)
                .map_err(|_| MirCodecError::CapabilityFailure)?;
            if length > MAX_CODEC_SEQUENCE_ELEMENTS as u64 {
                return Err(MirCodecError::LimitExceeded);
            }
            let mut bytes =
                vec![0; usize::try_from(length).map_err(|_| MirCodecError::LimitExceeded)?];
            runtime
                .immutable_bytes_read(*reference, 0, &mut bytes)
                .map_err(|_| MirCodecError::CapabilityFailure)?;
            MirCodecEvent::Bytes(bytes)
        }
        (Some(SemanticType::Primitive(PrimitiveType::Integer(kind))), MirValue::Integer(value))
            if value.kind() == *kind =>
        {
            MirCodecEvent::Integer(*value)
        }
        (Some(SemanticType::Primitive(PrimitiveType::Float32)), MirValue::Float(value))
            if value.kind() == FloatKind::Float32 =>
        {
            MirCodecEvent::Float(*value)
        }
        (Some(SemanticType::Primitive(PrimitiveType::Float64)), MirValue::Float(value))
            if value.kind() == FloatKind::Float64 =>
        {
            MirCodecEvent::Float(*value)
        }
        _ => return Err(MirCodecError::CapabilityFailure),
    };
    push_codec_event(events, event)
}

fn encode_codec_sequence<R: RuntimeAdapter>(
    element: TypeId,
    values: &[MirValue],
    events: &mut Vec<MirCodecEvent>,
    arena: &TypeArena,
    catalog: &[MirGeneratedCodecAdapter],
    runtime: &mut R,
    depth: u8,
) -> Result<(), MirCodecError> {
    if values.len() > MAX_CODEC_SEQUENCE_ELEMENTS || depth > MAX_CODEC_NESTING_DEPTH {
        return Err(MirCodecError::LimitExceeded);
    }
    push_codec_event(
        events,
        MirCodecEvent::SequenceStart(
            u32::try_from(values.len()).map_err(|_| MirCodecError::LimitExceeded)?,
        ),
    )?;
    for (index, value) in values.iter().enumerate() {
        push_codec_event(
            events,
            MirCodecEvent::Element(u16::try_from(index).map_err(|_| MirCodecError::LimitExceeded)?),
        )?;
        encode_codec_scalar(element, value, events, arena, catalog, runtime, depth)?;
    }
    push_codec_event(events, MirCodecEvent::SequenceEnd)?;
    Ok(())
}

fn decode_codec_value<R: RuntimeAdapter>(
    adapter: &MirGeneratedCodecAdapter,
    reader: &MirCodecReader,
    arena: &TypeArena,
    catalog: &[MirGeneratedCodecAdapter],
    runtime: &mut R,
    depth: u8,
) -> Result<MirValue, MirCodecError> {
    if depth > MAX_CODEC_NESTING_DEPTH {
        return Err(MirCodecError::LimitExceeded);
    }
    match next_codec_event(reader)? {
        MirCodecEvent::RecordStart(count) if usize::from(count) == adapter.members().len() => {
            let mut fields = Vec::with_capacity(adapter.members().len());
            for member in adapter.members() {
                if next_codec_event(reader)?
                    != (MirCodecEvent::Member {
                        ordinal: member.ordinal(),
                        label: member.name().to_owned(),
                    })
                {
                    return Err(MirCodecError::MalformedInput);
                }
                let MirGeneratedCodecMemberId::Field(field) = member.member() else {
                    return Err(MirCodecError::MalformedInput);
                };
                let type_id = member
                    .types()
                    .first()
                    .copied()
                    .ok_or(MirCodecError::MalformedInput)?;
                fields.push((
                    field,
                    decode_codec_scalar(type_id, reader, arena, catalog, runtime, depth + 1)?,
                ));
            }
            if next_codec_event(reader)? != MirCodecEvent::RecordEnd {
                return Err(MirCodecError::MalformedInput);
            }
            Ok(MirValue::Record {
                record: adapter.target().symbol(),
                fields,
            })
        }
        MirCodecEvent::EnumCase {
            ordinal,
            label,
            discriminant,
        } => {
            let member = adapter
                .members()
                .iter()
                .find(|member| {
                    member.ordinal() == ordinal
                        && member.name() == label
                        && member.discriminant() == Some(discriminant)
                })
                .ok_or(MirCodecError::MalformedInput)?;
            let MirGeneratedCodecMemberId::EnumCase(case) = member.member() else {
                return Err(MirCodecError::MalformedInput);
            };
            Ok(MirValue::Enum {
                definition: adapter.target().symbol(),
                case,
                discriminant,
            })
        }
        MirCodecEvent::UnionStart {
            ordinal,
            label,
            payload_count,
        } => {
            let member = adapter
                .members()
                .iter()
                .find(|member| member.ordinal() == ordinal && member.name() == label)
                .ok_or(MirCodecError::MalformedInput)?;
            let MirGeneratedCodecMemberId::UnionCase(case) = member.member() else {
                return Err(MirCodecError::MalformedInput);
            };
            if usize::from(payload_count) != member.types().len() {
                return Err(MirCodecError::MalformedInput);
            }
            let mut arguments = Vec::with_capacity(member.types().len());
            for (ordinal, type_id) in member.types().iter().enumerate() {
                if next_codec_event(reader)?
                    != MirCodecEvent::Payload(
                        u16::try_from(ordinal).map_err(|_| MirCodecError::LimitExceeded)?,
                    )
                {
                    return Err(MirCodecError::MalformedInput);
                }
                arguments.push(decode_codec_scalar(
                    *type_id,
                    reader,
                    arena,
                    catalog,
                    runtime,
                    depth + 1,
                )?);
            }
            if next_codec_event(reader)? != MirCodecEvent::UnionEnd {
                return Err(MirCodecError::MalformedInput);
            }
            Ok(MirValue::Union {
                union: adapter.target().symbol(),
                case,
                arguments,
            })
        }
        _ => Err(MirCodecError::MalformedInput),
    }
}

fn decode_codec_scalar<R: RuntimeAdapter>(
    type_id: TypeId,
    reader: &MirCodecReader,
    arena: &TypeArena,
    catalog: &[MirGeneratedCodecAdapter],
    runtime: &mut R,
    depth: u8,
) -> Result<MirValue, MirCodecError> {
    if depth > MAX_CODEC_NESTING_DEPTH {
        return Err(MirCodecError::LimitExceeded);
    }
    if let Some(adapter) = catalog
        .iter()
        .find(|adapter| adapter.target_type() == type_id)
    {
        return decode_codec_value(adapter, reader, arena, catalog, runtime, depth);
    }
    let event = next_codec_event(reader)?;
    match (arena.get(type_id), event) {
        (Some(SemanticType::Tuple(types)), MirCodecEvent::TupleStart(count))
            if usize::from(count) == types.len() =>
        {
            let mut values = Vec::with_capacity(types.len());
            for (index, type_id) in types.iter().enumerate() {
                if next_codec_event(reader)?
                    != MirCodecEvent::Element(
                        u16::try_from(index).map_err(|_| MirCodecError::LimitExceeded)?,
                    )
                {
                    return Err(MirCodecError::MalformedInput);
                }
                values.push(decode_codec_scalar(
                    *type_id,
                    reader,
                    arena,
                    catalog,
                    runtime,
                    depth + 1,
                )?);
            }
            if next_codec_event(reader)? != MirCodecEvent::TupleEnd {
                return Err(MirCodecError::MalformedInput);
            }
            Ok(MirValue::Tuple(values))
        }
        (Some(SemanticType::Array(element)), MirCodecEvent::SequenceStart(count)) => {
            decode_codec_sequence(*element, count, reader, arena, catalog, runtime, depth + 1)
                .map(MirValue::Array)
        }
        (
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }),
            MirCodecEvent::SequenceStart(count),
        ) if definition.raw() == 101 && arguments.len() == 1 => decode_codec_sequence(
            arguments[0],
            count,
            reader,
            arena,
            catalog,
            runtime,
            depth + 1,
        )
        .map(MirValue::List),
        (Some(SemanticType::Union(types)), MirCodecEvent::OptionalAbsent)
            if optional_payload_type(types, arena).is_some() =>
        {
            Ok(MirValue::Nil)
        }
        (Some(SemanticType::Union(types)), MirCodecEvent::OptionalPresent) => {
            let payload =
                optional_payload_type(types, arena).ok_or(MirCodecError::MalformedInput)?;
            decode_codec_scalar(payload, reader, arena, catalog, runtime, depth + 1)
        }
        (Some(SemanticType::Primitive(PrimitiveType::Boolean)), MirCodecEvent::Boolean(value)) => {
            Ok(MirValue::Boolean(value))
        }
        (Some(SemanticType::Primitive(PrimitiveType::String)), MirCodecEvent::String(value)) => {
            Ok(MirValue::String(value))
        }
        (
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }),
            MirCodecEvent::Bytes(bytes),
        ) if definition.raw() == 0 && arguments.is_empty() => runtime
            .allocate_immutable_bytes(&bytes)
            .map(MirValue::Bytes)
            .map_err(|_| MirCodecError::CapabilityFailure),
        (
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))),
            MirCodecEvent::Integer(value),
        ) if value.kind() == *kind => Ok(MirValue::Integer(value)),
        (Some(SemanticType::Primitive(PrimitiveType::Float32)), MirCodecEvent::Float(value))
            if value.kind() == FloatKind::Float32 =>
        {
            Ok(MirValue::Float(value))
        }
        (Some(SemanticType::Primitive(PrimitiveType::Float64)), MirCodecEvent::Float(value))
            if value.kind() == FloatKind::Float64 =>
        {
            Ok(MirValue::Float(value))
        }
        _ => Err(MirCodecError::MalformedInput),
    }
}

fn decode_codec_sequence<R: RuntimeAdapter>(
    element: TypeId,
    count: u32,
    reader: &MirCodecReader,
    arena: &TypeArena,
    catalog: &[MirGeneratedCodecAdapter],
    runtime: &mut R,
    depth: u8,
) -> Result<Vec<MirValue>, MirCodecError> {
    let count = usize::try_from(count).map_err(|_| MirCodecError::LimitExceeded)?;
    if count > MAX_CODEC_SEQUENCE_ELEMENTS || depth > MAX_CODEC_NESTING_DEPTH {
        return Err(MirCodecError::LimitExceeded);
    }
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        if next_codec_event(reader)?
            != MirCodecEvent::Element(
                u16::try_from(index).map_err(|_| MirCodecError::LimitExceeded)?,
            )
        {
            return Err(MirCodecError::MalformedInput);
        }
        values.push(decode_codec_scalar(
            element, reader, arena, catalog, runtime, depth,
        )?);
    }
    if next_codec_event(reader)? != MirCodecEvent::SequenceEnd {
        return Err(MirCodecError::MalformedInput);
    }
    Ok(values)
}

fn optional_payload_type(types: &[TypeId], arena: &TypeArena) -> Option<TypeId> {
    if types.len() != 2
        || !types.iter().any(|type_id| {
            arena.get(*type_id) == Some(&SemanticType::Primitive(PrimitiveType::Nil))
        })
    {
        return None;
    }
    types
        .iter()
        .copied()
        .find(|type_id| arena.get(*type_id) != Some(&SemanticType::Primitive(PrimitiveType::Nil)))
}

fn next_codec_event(reader: &MirCodecReader) -> Result<MirCodecEvent, MirCodecError> {
    let position = reader.position.get();
    let event = reader
        .events
        .get(position)
        .cloned()
        .ok_or(MirCodecError::MalformedInput)?;
    reader.position.set(position + 1);
    Ok(event)
}

#[cfg(test)]
mod codec_tests {
    use super::*;
    use pop_foundation::{BubbleId, BuiltinTypeId, EnumCaseId, FieldId, ModuleId, UnionCaseId};
    use pop_mir::{MirGeneratedCodecAdapter, MirGeneratedCodecMember, MirGeneratedCodecMemberId};
    use pop_resolve::Visibility;

    fn adapter(
        target: SymbolId,
        target_type: TypeId,
        members: Vec<MirGeneratedCodecMember>,
    ) -> MirGeneratedCodecAdapter {
        MirGeneratedCodecAdapter::new(
            SymbolId::from_raw(target.raw() + 10),
            SymbolIdentity::new(BubbleId::from_raw(0), target),
            ModuleId::from_raw(0),
            Visibility::Public,
            "ValueSchema".to_owned(),
            "Value".to_owned(),
            target_type,
            TypeId::from_raw(999),
            1,
            "0".repeat(64),
            members,
        )
    }

    #[test]
    fn generated_codec_record_enum_union_events_round_trip_and_reject_tamper() {
        let mut arena = TypeArena::new();
        let mut runtime = ReferenceRuntimeAdapter::default();
        let text = arena.source_type("String").expect("String");
        let integer = arena.source_type("Int").expect("Int");
        let record_type = arena
            .intern(SemanticType::Record(vec![
                ("name".to_owned(), text),
                ("age".to_owned(), integer),
            ]))
            .expect("record type");
        let record = adapter(
            SymbolId::from_raw(1),
            record_type,
            vec![
                MirGeneratedCodecMember::new(
                    0,
                    "name".to_owned(),
                    MirGeneratedCodecMemberId::Field(FieldId::from_raw(0)),
                    vec![text],
                    None,
                ),
                MirGeneratedCodecMember::new(
                    1,
                    "age".to_owned(),
                    MirGeneratedCodecMemberId::Field(FieldId::from_raw(1)),
                    vec![integer],
                    None,
                ),
            ],
        );
        let value = MirValue::Record {
            record: SymbolId::from_raw(1),
            fields: vec![
                (FieldId::from_raw(0), MirValue::String("Ada".to_owned())),
                (
                    FieldId::from_raw(1),
                    MirValue::Integer(
                        IntegerValue::parse_decimal("42", IntegerKind::Int64).expect("Int"),
                    ),
                ),
            ],
        };
        let mut events = Vec::new();
        encode_codec_value(
            &record,
            &value,
            &mut events,
            &arena,
            std::slice::from_ref(&record),
            &mut runtime,
            0,
        )
        .expect("encode record");
        assert_eq!(
            decode_codec_value(
                &record,
                &MirCodecReader::new(events.clone()),
                &arena,
                std::slice::from_ref(&record),
                &mut runtime,
                0,
            ),
            Ok(value)
        );
        let MirCodecEvent::Member { label, .. } = &mut events[1] else {
            panic!("member")
        };
        *label = "wrong".to_owned();
        assert_eq!(
            decode_codec_value(
                &record,
                &MirCodecReader::new(events),
                &arena,
                std::slice::from_ref(&record),
                &mut runtime,
                0,
            ),
            Err(MirCodecError::MalformedInput)
        );

        let enumeration = adapter(
            SymbolId::from_raw(2),
            TypeId::from_raw(700),
            vec![MirGeneratedCodecMember::new(
                0,
                "Ready".to_owned(),
                MirGeneratedCodecMemberId::EnumCase(EnumCaseId::from_raw(0)),
                Vec::new(),
                Some(7),
            )],
        );
        let enum_value = MirValue::Enum {
            definition: SymbolId::from_raw(2),
            case: EnumCaseId::from_raw(0),
            discriminant: 7,
        };
        let mut enum_events = Vec::new();
        encode_codec_value(
            &enumeration,
            &enum_value,
            &mut enum_events,
            &arena,
            std::slice::from_ref(&enumeration),
            &mut runtime,
            0,
        )
        .expect("encode enum");
        assert_eq!(
            decode_codec_value(
                &enumeration,
                &MirCodecReader::new(enum_events),
                &arena,
                std::slice::from_ref(&enumeration),
                &mut runtime,
                0,
            ),
            Ok(enum_value)
        );

        let union = adapter(
            SymbolId::from_raw(3),
            TypeId::from_raw(701),
            vec![MirGeneratedCodecMember::new(
                0,
                "Named".to_owned(),
                MirGeneratedCodecMemberId::UnionCase(UnionCaseId::from_raw(0)),
                vec![text],
                None,
            )],
        );
        let union_value = MirValue::Union {
            union: SymbolId::from_raw(3),
            case: UnionCaseId::from_raw(0),
            arguments: vec![MirValue::String("Pop".to_owned())],
        };
        let mut union_events = Vec::new();
        encode_codec_value(
            &union,
            &union_value,
            &mut union_events,
            &arena,
            std::slice::from_ref(&union),
            &mut runtime,
            0,
        )
        .expect("encode union");
        assert_eq!(
            decode_codec_value(
                &union,
                &MirCodecReader::new(union_events),
                &arena,
                std::slice::from_ref(&union),
                &mut runtime,
                0,
            ),
            Ok(union_value)
        );
    }

    #[test]
    fn generated_codec_recurses_through_exact_nested_catalog_and_closed_containers() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let text = arena.source_type("String").expect("String");
        let nil = arena.source_type("nil").expect("nil");
        let array = arena.intern(SemanticType::Array(integer)).expect("array");
        let list = arena
            .intern(SemanticType::Builtin {
                definition: BuiltinTypeId::from_raw(101),
                arguments: vec![integer],
            })
            .expect("list");
        let optional = arena
            .intern(SemanticType::Union(vec![nil, text]))
            .expect("optional");
        let tuple = arena
            .intern(SemanticType::Tuple(vec![array, list, optional]))
            .expect("tuple");
        let inner_type = arena
            .intern(SemanticType::Record(vec![("items".to_owned(), tuple)]))
            .expect("inner record");
        let outer_type = arena
            .intern(SemanticType::Record(vec![("inner".to_owned(), inner_type)]))
            .expect("outer record");
        let inner = adapter(
            SymbolId::from_raw(20),
            inner_type,
            vec![MirGeneratedCodecMember::new(
                0,
                "items".to_owned(),
                MirGeneratedCodecMemberId::Field(FieldId::from_raw(20)),
                vec![tuple],
                None,
            )],
        );
        let outer = adapter(
            SymbolId::from_raw(21),
            outer_type,
            vec![MirGeneratedCodecMember::new(
                0,
                "inner".to_owned(),
                MirGeneratedCodecMemberId::Field(FieldId::from_raw(21)),
                vec![inner_type],
                None,
            )],
        );
        let number = || {
            MirValue::Integer(IntegerValue::parse_decimal("7", IntegerKind::Int64).expect("Int"))
        };
        let value = MirValue::Record {
            record: SymbolId::from_raw(21),
            fields: vec![(
                FieldId::from_raw(21),
                MirValue::Record {
                    record: SymbolId::from_raw(20),
                    fields: vec![(
                        FieldId::from_raw(20),
                        MirValue::Tuple(vec![
                            MirValue::Array(vec![number()]),
                            MirValue::List(vec![number()]),
                            MirValue::String("Pop".to_owned()),
                        ]),
                    )],
                },
            )],
        };
        let catalog = vec![outer.clone(), inner];
        let mut runtime = ReferenceRuntimeAdapter::default();
        let mut events = Vec::new();
        encode_codec_value(
            &outer,
            &value,
            &mut events,
            &arena,
            &catalog,
            &mut runtime,
            0,
        )
        .expect("encode nested value");
        assert!(events.contains(&MirCodecEvent::TupleStart(3)));
        assert!(events.contains(&MirCodecEvent::OptionalPresent));
        assert_eq!(
            decode_codec_value(
                &outer,
                &MirCodecReader::new(events),
                &arena,
                &catalog,
                &mut runtime,
                0,
            ),
            Ok(value)
        );
    }

    #[test]
    fn generated_codec_bytes_and_sequence_limits_are_typed() {
        let mut arena = TypeArena::new();
        let bytes_type = arena
            .intern(SemanticType::Builtin {
                definition: BuiltinTypeId::from_raw(0),
                arguments: Vec::new(),
            })
            .expect("Bytes");
        let integer = arena.source_type("Int").expect("Int");
        let array = arena.intern(SemanticType::Array(integer)).expect("array");
        let mut runtime = ReferenceRuntimeAdapter::default();
        let reference = runtime
            .allocate_immutable_bytes(&[0, 1, 255])
            .expect("allocate Bytes");
        let mut events = Vec::new();
        encode_codec_scalar(
            bytes_type,
            &MirValue::Bytes(reference),
            &mut events,
            &arena,
            &[],
            &mut runtime,
            0,
        )
        .expect("encode Bytes");
        assert_eq!(events, vec![MirCodecEvent::Bytes(vec![0, 1, 255])]);
        let decoded = decode_codec_scalar(
            bytes_type,
            &MirCodecReader::new(events),
            &arena,
            &[],
            &mut runtime,
            0,
        )
        .expect("decode Bytes");
        let MirValue::Bytes(decoded) = decoded else {
            panic!("decoded Bytes")
        };
        let mut payload = [0; 3];
        runtime
            .immutable_bytes_read(decoded, 0, &mut payload)
            .expect("read Bytes");
        assert_eq!(payload, [0, 1, 255]);

        assert_eq!(
            decode_codec_scalar(
                array,
                &MirCodecReader::new(vec![MirCodecEvent::SequenceStart(65_536)]),
                &arena,
                &[],
                &mut runtime,
                0,
            ),
            Err(MirCodecError::LimitExceeded)
        );

        let value =
            MirValue::Integer(IntegerValue::parse_decimal("7", IntegerKind::Int64).expect("Int"));
        let mut bounded_events = Vec::new();
        assert_eq!(
            encode_codec_scalar(
                array,
                &MirValue::Array(vec![value; 32_768]),
                &mut bounded_events,
                &arena,
                &[],
                &mut runtime,
                0,
            ),
            Err(MirCodecError::LimitExceeded)
        );
        assert_eq!(bounded_events.len(), MAX_CODEC_EVENTS);
        let writer = crate::values::MirCodecWriter::new();
        assert!(writer.append_within_limit(vec![MirCodecEvent::Boolean(false)], MAX_CODEC_EVENTS));
        assert_eq!(
            writer.events(),
            vec![MirCodecEvent::Boolean(false)],
            "over-limit temporary events must never replace committed tape"
        );
        assert!(writer.append_within_limit(vec![MirCodecEvent::Boolean(true)], MAX_CODEC_EVENTS));
        assert_eq!(
            writer.events(),
            vec![MirCodecEvent::Boolean(false), MirCodecEvent::Boolean(true)]
        );

        assert_eq!(
            encode_codec_scalar(
                array,
                &MirValue::Array(vec![MirValue::Nil; 65_536]),
                &mut Vec::new(),
                &arena,
                &[],
                &mut runtime,
                0,
            ),
            Err(MirCodecError::LimitExceeded)
        );
    }
}

fn runtime_callback_site(
    owner: SymbolId,
    site: MirFfiCallbackSiteId,
) -> Result<FfiCallbackSiteId, ExecutionError> {
    FfiCallbackSiteId::new((u64::from(owner.raw()) << 32) | u64::from(site.raw()))
        .ok_or(ExecutionError::InvalidControlFlow)
}

fn require_foreign_abi_values(
    mir: &MirBubble,
    arena: &TypeArena,
    expected: &[TypeId],
    layouts: &[Option<pop_runtime_interface::FfiAbiLayoutId>],
    values: &[MirValue],
) -> Result<(), ExecutionError> {
    if expected.len() != values.len() || layouts.len() != values.len() {
        return Err(ExecutionError::WrongArity);
    }
    for ((expected, layout), value) in expected.iter().zip(layouts).zip(values) {
        let matches = if let Some(layout) = layout {
            let layout = mir
                .ffi_layouts()
                .get(*layout)
                .filter(|layout| layout.element() == *expected)
                .ok_or(ExecutionError::InvalidControlFlow)?;
            foreign_layout_value_matches(mir, arena, layout, value)?
        } else {
            foreign_scalar_value_matches(mir, arena, *expected, value)?
        };
        if !matches {
            return Err(ExecutionError::TypeMismatch);
        }
    }
    Ok(())
}

fn foreign_scalar_value_matches(
    mir: &MirBubble,
    arena: &TypeArena,
    expected: TypeId,
    value: &MirValue,
) -> Result<bool, ExecutionError> {
    Ok(match arena.get(expected) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
            matches!(value, MirValue::Integer(integer) if integer.kind() == *kind)
        }
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
            matches!(value, MirValue::Float(float) if float.kind() == FloatKind::Float32)
        }
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
            matches!(value, MirValue::Float(float) if float.kind() == FloatKind::Float64)
        }
        Some(SemanticType::Builtin { definition, .. })
            if is_ffi_integer_abi_builtin_type(*definition) =>
        {
            let kind = integer_kind_for_type(expected, mir.ffi_layouts(), arena)?;
            matches!(value, MirValue::Integer(integer) if integer.kind() == kind)
        }
        Some(SemanticType::Builtin { definition, .. })
            if is_ffi_pointer_type_constructor(*definition) =>
        {
            matches!(value, MirValue::FfiPointer(_))
                || (*definition == FFI_OPTIONAL_POINTER_TYPE_ID
                    || *definition == FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID)
                    && matches!(value, MirValue::Nil)
        }
        Some(SemanticType::Builtin { definition, .. })
            if is_ffi_function_type_constructor(*definition) =>
        {
            matches!(value, MirValue::FfiFunction(_))
                || definition.raw() == 203 && matches!(value, MirValue::Nil)
        }
        Some(SemanticType::Builtin { definition, .. }) if *definition == FFI_HANDLE_TYPE_ID => {
            matches!(value, MirValue::FfiHandle(handle) if *handle != 0)
        }
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) if *definition == FFI_CALLBACK_CONTEXT_TYPE_ID && arguments.is_empty() => {
            matches!(value, MirValue::FfiPointer(_))
        }
        _ => return Err(ExecutionError::InvalidControlFlow),
    })
}

fn callback_abi_value_matches(
    mir: &MirBubble,
    arena: &TypeArena,
    expected: TypeId,
    value: &MirValue,
) -> Result<bool, ExecutionError> {
    let mut layouts = mir
        .ffi_layouts()
        .entries()
        .iter()
        .filter(|layout| layout.element() == expected);
    let first = layouts.next();
    if layouts.next().is_some() {
        return Err(ExecutionError::InvalidControlFlow);
    }
    match first {
        Some(layout) => foreign_layout_value_matches(mir, arena, layout, value),
        None => foreign_scalar_value_matches(mir, arena, expected, value),
    }
}

fn foreign_layout_value_matches(
    mir: &MirBubble,
    arena: &TypeArena,
    layout: &MirFfiLayout,
    value: &MirValue,
) -> Result<bool, ExecutionError> {
    Ok(match layout.value_class() {
        MirFfiValueClass::Integer => {
            let kind = integer_kind_for_type(layout.element(), mir.ffi_layouts(), arena)?;
            matches!(value, MirValue::Integer(integer) if integer.kind() == kind)
        }
        MirFfiValueClass::Float => match layout.size() {
            4 => matches!(value, MirValue::Float(float) if float.kind() == FloatKind::Float32),
            8 => matches!(value, MirValue::Float(float) if float.kind() == FloatKind::Float64),
            _ => return Err(ExecutionError::InvalidControlFlow),
        },
        MirFfiValueClass::Pointer
        | MirFfiValueClass::FunctionPointer
        | MirFfiValueClass::Handle => {
            foreign_scalar_value_matches(mir, arena, layout.element(), value)?
        }
        MirFfiValueClass::Record(plan) => {
            let Some(expected_record) =
                mir.declarations()
                    .iter()
                    .find_map(|declaration| match declaration.kind() {
                        pop_mir::MirDeclarationKind::Record(record)
                            if record.type_id() == layout.element() =>
                        {
                            Some(declaration.symbol())
                        }
                        _ => None,
                    })
            else {
                return Err(ExecutionError::InvalidControlFlow);
            };
            let MirValue::Record { record, fields } = value else {
                return Ok(false);
            };
            if *record != expected_record || fields.len() != plan.len() {
                return Ok(false);
            }
            for field in plan {
                let Some(value) = fields
                    .iter()
                    .find_map(|(identity, value)| (*identity == field.field()).then_some(value))
                else {
                    return Ok(false);
                };
                let child = mir
                    .ffi_layouts()
                    .get(field.layout())
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                if !foreign_layout_value_matches(mir, arena, child, value)? {
                    return Ok(false);
                }
            }
            true
        }
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionLimits {
    maximum_steps: u64,
    maximum_call_depth: u32,
}

impl ExecutionLimits {
    #[must_use]
    pub const fn new(maximum_steps: u64, maximum_call_depth: u32) -> Self {
        Self {
            maximum_steps,
            maximum_call_depth,
        }
    }
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self::new(1_000_000, 256)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    UnknownFunction(SymbolId),
    UnsupportedForeignFunction(SymbolId),
    UnsupportedFfiCallback {
        function: u64,
        context: ForeignAddress,
    },
    UnknownReferencedFunction(SymbolIdentity),
    WrongArity,
    TypeMismatch,
    MissingValue(ValueId),
    IntegerOverflow,
    DivisionByZero,
    NumericConversion,
    Runtime(RuntimeFailure),
    StepLimit,
    CallDepthLimit,
    ReachedUnreachable,
    InvalidControlFlow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForeignAdapterRegistrationError {
    UnknownForeignFunction(SymbolId),
    SignatureMismatch(SymbolId),
    Duplicate(SymbolId),
}

pub trait FfiCallbackInvoker {
    /// Invokes one exact function/context pair published by the interpreter.
    ///
    /// # Errors
    ///
    /// Rejects unavailable, stale, closed, mismatched, or ill-typed pairs
    /// before managed callback execution.
    fn invoke(
        &mut self,
        function: &MirValue,
        context: &MirValue,
        arguments: &[MirValue],
    ) -> Result<Vec<MirValue>, ExecutionError>;
}

type ForeignAdapterFunction = dyn FnMut(&[MirValue], &mut dyn FfiCallbackInvoker) -> Result<Vec<MirValue>, ExecutionError>
    + 'static;

pub struct TypedForeignAdapter {
    symbol: SymbolId,
    parameters: Vec<TypeId>,
    results: Vec<TypeId>,
    function: Box<ForeignAdapterFunction>,
}

impl TypedForeignAdapter {
    #[must_use]
    pub fn new<F>(
        symbol: SymbolId,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        function: F,
    ) -> Self
    where
        F: FnMut(&[MirValue]) -> Result<Vec<MirValue>, RuntimeFailure> + 'static,
    {
        let mut function = function;
        Self {
            symbol,
            parameters,
            results,
            function: Box::new(move |arguments, _| {
                function(arguments).map_err(ExecutionError::Runtime)
            }),
        }
    }

    /// Creates an exact foreign adapter that may synchronously invoke a
    /// compiler-proven callback function/context pair.
    #[must_use]
    pub fn new_with_callbacks<F>(
        symbol: SymbolId,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        function: F,
    ) -> Self
    where
        F: FnMut(&[MirValue], &mut dyn FfiCallbackInvoker) -> Result<Vec<MirValue>, ExecutionError>
            + 'static,
    {
        Self {
            symbol,
            parameters,
            results,
            function: Box::new(function),
        }
    }
}

pub struct MirInterpreter<'mir, R = ReferenceRuntimeAdapter> {
    mir: &'mir MirBubble,
    arena: &'mir TypeArena,
    limits: ExecutionLimits,
    runtime: RefCell<R>,
    foreign_adapters: RefCell<BTreeMap<SymbolId, TypedForeignAdapter>>,
    ffi_callbacks: RefCell<BTreeMap<FfiCallbackRegistrationId, InterpreterCallback>>,
}

impl<'mir> MirInterpreter<'mir, ReferenceRuntimeAdapter> {
    /// Accepts only MIR that passes the canonical verifier.
    ///
    /// # Errors
    ///
    /// Returns every verifier failure before execution can begin.
    pub fn new(
        mir: &'mir MirBubble,
        arena: &'mir TypeArena,
    ) -> Result<Self, Vec<MirVerificationError>> {
        verify_mir_bubble(mir, arena)?;
        Ok(Self {
            mir,
            arena,
            limits: ExecutionLimits::default(),
            runtime: RefCell::new(ReferenceRuntimeAdapter::default()),
            foreign_adapters: RefCell::new(BTreeMap::new()),
            ffi_callbacks: RefCell::new(BTreeMap::new()),
        })
    }
}

impl<'mir, R: RuntimeAdapter> MirInterpreter<'mir, R> {
    /// Accepts verified MIR with an explicitly selected PLRI adapter.
    ///
    /// # Errors
    ///
    /// Returns all canonical MIR verification failures before retaining the
    /// runtime adapter.
    pub fn with_runtime(
        mir: &'mir MirBubble,
        arena: &'mir TypeArena,
        runtime: R,
    ) -> Result<Self, Vec<MirVerificationError>> {
        verify_mir_bubble(mir, arena)?;
        Ok(Self {
            mir,
            arena,
            limits: ExecutionLimits::default(),
            runtime: RefCell::new(runtime),
            foreign_adapters: RefCell::new(BTreeMap::new()),
            ffi_callbacks: RefCell::new(BTreeMap::new()),
        })
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Installs one test-only foreign adapter after matching its exact resolved
    /// symbol and static parameter/result packs.
    ///
    /// # Errors
    ///
    /// Rejects unknown identities, signature drift, and duplicate authority.
    pub fn with_foreign_adapter(
        mut self,
        adapter: TypedForeignAdapter,
    ) -> Result<Self, ForeignAdapterRegistrationError> {
        let Some(declaration) = self
            .mir
            .foreign_functions()
            .iter()
            .find(|declaration| declaration.symbol() == adapter.symbol)
        else {
            return Err(ForeignAdapterRegistrationError::UnknownForeignFunction(
                adapter.symbol,
            ));
        };
        if declaration.parameters() != adapter.parameters
            || declaration.results() != adapter.results
        {
            return Err(ForeignAdapterRegistrationError::SignatureMismatch(
                adapter.symbol,
            ));
        }
        if self
            .foreign_adapters
            .get_mut()
            .contains_key(&adapter.symbol)
        {
            return Err(ForeignAdapterRegistrationError::Duplicate(
                declaration.symbol(),
            ));
        }
        self.foreign_adapters
            .get_mut()
            .insert(adapter.symbol, adapter);
        Ok(self)
    }

    #[must_use]
    pub fn runtime(&self) -> Ref<'_, R> {
        self.runtime.borrow()
    }

    /// Calls one MIR function by its already-resolved stable symbol.
    ///
    /// # Errors
    ///
    /// Returns deterministic type, arithmetic, control-flow, or resource
    /// failures. It never performs runtime lookup from a source string.
    pub fn call(
        &self,
        function: SymbolId,
        arguments: &[MirValue],
    ) -> Result<Vec<MirValue>, ExecutionError> {
        let arguments: Vec<_> = arguments
            .iter()
            .cloned()
            .map(RuntimeValue::visible)
            .collect();
        let mut runtime = self.runtime.borrow_mut();
        let mut foreign_adapters = self.foreign_adapters.borrow_mut();
        let mut ffi_callbacks = self.ffi_callbacks.borrow_mut();
        Engine {
            mir: self.mir,
            arena: self.arena,
            limits: self.limits,
            steps: 0,
            depth: 0,
            runtime: &mut *runtime,
            foreign_adapters: &mut foreign_adapters,
            root_handles: BTreeMap::new(),
            ffi_handles: BTreeMap::new(),
            ffi_buffer_borrows: BTreeMap::new(),
            ffi_bytes_borrows: BTreeMap::new(),
            ffi_callbacks: &mut ffi_callbacks,
            pin_handles: BTreeMap::new(),
            private_values: BTreeMap::new(),
            next_private_value: u32::MAX,
            active_captures: None,
            active_task: None,
        }
        .call(function, &arguments)
        .map(|values| values.into_iter().map(|value| value.visible).collect())
    }
}

struct Engine<'mir, 'runtime, R> {
    mir: &'mir MirBubble,
    arena: &'mir TypeArena,
    limits: ExecutionLimits,
    steps: u64,
    depth: u32,
    runtime: &'runtime mut R,
    foreign_adapters: &'runtime mut BTreeMap<SymbolId, TypedForeignAdapter>,
    root_handles: BTreeMap<ValueId, RootHandle>,
    ffi_handles: BTreeMap<RootHandle, RuntimeValue>,
    ffi_buffer_borrows: BTreeMap<BorrowRegionId, FfiBufferBorrowId>,
    ffi_bytes_borrows: BTreeMap<BorrowRegionId, FfiBytesBorrowState>,
    ffi_callbacks: &'runtime mut BTreeMap<FfiCallbackRegistrationId, InterpreterCallback>,
    pin_handles: BTreeMap<ValueId, PinHandle>,
    private_values: BTreeMap<SymbolId, PrivateValue>,
    next_private_value: u32,
    active_captures: Option<Rc<RefCell<Vec<RuntimeValue>>>>,
    active_task: Option<TaskId>,
}

#[derive(Clone, Copy)]
struct FfiBytesBorrowState {
    owner: ManagedReference,
    borrow: FfiBytesBorrowId,
    length: u64,
}

#[derive(Clone)]
struct InterpreterCallback {
    registration: FfiCallbackRegistration,
    site: FfiCallbackSiteId,
    target: InterpreterCallbackTarget,
    environment: ManagedReference,
    closed: bool,
}

#[derive(Clone)]
enum InterpreterCallbackTarget {
    Closure {
        owner: SymbolId,
        function: NestedFunctionId,
        captures: Rc<RefCell<Vec<RuntimeValue>>>,
    },
}

enum PrivateValue {
    Cell(Rc<RefCell<RuntimeValue>>),
    Closure {
        owner: SymbolId,
        function: NestedFunctionId,
        captures: Rc<RefCell<Vec<RuntimeValue>>>,
    },
    Iterator {
        source: RuntimeValue,
        expected_length: usize,
        position: usize,
        range_current: Option<pop_types::IntegerValue>,
        range_started: bool,
    },
    Task(Rc<RefCell<TaskState>>),
    CancellationSource(Rc<RefCell<CancellationState>>),
    CancellationToken(Rc<RefCell<CancellationState>>),
    TaskGroup(Rc<RefCell<InterpreterTaskGroup>>),
}

#[derive(Clone)]
struct CancellationState {
    token: CancellationTokenId,
    requested: bool,
}

struct InterpreterTaskGroup {
    lifecycle: TaskGroupLifecycle,
    cancellation: Rc<RefCell<CancellationState>>,
    children: BTreeMap<TaskId, SymbolId>,
    reference: ManagedReference,
}

#[derive(Clone)]
enum TaskTarget {
    Direct(SymbolId),
    Referenced(SymbolIdentity),
    Indirect(RuntimeValue),
    Group { body: RuntimeValue, group: SymbolId },
}

#[derive(Clone)]
struct TaskState {
    lifecycle: TaskLifecycle,
    completion_type: TypeId,
    execution: TaskExecution,
}

#[derive(Clone)]
enum TaskExecution {
    Created {
        target: TaskTarget,
        arguments: Vec<RuntimeValue>,
        owner: pop_runtime_interface::ManagedReference,
        completion_slot: ObjectSlot,
    },
    Running,
    Completed(Result<RuntimeValue, ExecutionError>),
}

impl<R: RuntimeAdapter> Engine<'_, '_, R> {
    fn call(
        &mut self,
        symbol: SymbolId,
        arguments: &[RuntimeValue],
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let function = self
            .mir
            .functions()
            .iter()
            .find(|function| function.symbol() == symbol)
            .ok_or(ExecutionError::UnknownFunction(symbol))?;
        if function.parameters().len() != arguments.len() {
            return Err(ExecutionError::WrongArity);
        }
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(ExecutionError::CallDepthLimit)?;
        if self.depth > self.limits.maximum_call_depth {
            return Err(ExecutionError::CallDepthLimit);
        }
        let result = self.execute(
            function.parameters(),
            function.results(),
            function.blocks(),
            arguments,
            None,
        );
        self.depth -= 1;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_foreign_call(
        &mut self,
        symbol: SymbolId,
        arguments: &[ValueId],
        roots: &[ValueId],
        safe_point: pop_runtime_interface::SafePointId,
        effects: pop_mir::MirEffectSummary,
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let declaration = self
            .mir
            .foreign_functions()
            .iter()
            .find(|declaration| declaration.symbol() == symbol)
            .ok_or(ExecutionError::UnsupportedForeignFunction(symbol))?;
        let visible_arguments = arguments
            .iter()
            .map(|argument| value(values, *argument).map(|value| value.visible.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        require_foreign_abi_values(
            self.mir,
            self.arena,
            declaration.parameters(),
            declaration.parameter_layouts(),
            &visible_arguments,
        )?;
        if !self.foreign_adapters.contains_key(&symbol) {
            return Err(ExecutionError::UnsupportedForeignFunction(symbol));
        }
        let published_values = roots
            .iter()
            .map(|root| value(values, *root).map(|value| value.reference))
            .collect::<Result<Vec<_>, _>>()?;
        let stack_map = StackMap::new(
            safe_point,
            (0..roots.len())
                .map(|slot| {
                    u32::try_from(slot)
                        .map(RootSlot::new)
                        .map_err(|_| ExecutionError::InvalidControlFlow)
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| ExecutionError::InvalidControlFlow)?;
        let mut publication = RootPublication::new(stack_map, published_values)
            .map_err(|_| ExecutionError::InvalidControlFlow)?;
        let mode = if effects.contains(pop_mir::MirEffect::Blocks) {
            ForeignCallMode::Blocking
        } else {
            ForeignCallMode::BoundedNonblocking
        };
        let transition = self
            .runtime
            .enter_foreign(&mut publication, mode)
            .map_err(ExecutionError::Runtime)?;
        let mut adapter = self
            .foreign_adapters
            .remove(&symbol)
            .ok_or(ExecutionError::UnsupportedForeignFunction(symbol))?;
        let invocation = (adapter.function)(&visible_arguments, self);
        if self.foreign_adapters.insert(symbol, adapter).is_some() {
            return Err(ExecutionError::InvalidControlFlow);
        }
        self.runtime
            .leave_foreign(transition, &mut publication)
            .map_err(ExecutionError::Runtime)?;
        install_published_relocations(roots, &publication, values)?;
        let returned = invocation?;
        require_foreign_abi_values(
            self.mir,
            self.arena,
            declaration.results(),
            declaration.result_layouts(),
            &returned,
        )?;
        Ok(returned.into_iter().map(RuntimeValue::visible).collect())
    }

    fn execute(
        &mut self,
        parameters: &[TypeId],
        results: &[TypeId],
        blocks: &[pop_mir::MirBlock],
        arguments: &[RuntimeValue],
        captures: Option<Rc<RefCell<Vec<RuntimeValue>>>>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        require_runtime_numeric_types(self.arena, parameters, arguments)?;
        let previous_captures = std::mem::replace(&mut self.active_captures, captures);
        let result = self.execute_blocks(results, blocks, arguments);
        self.active_captures = previous_captures;
        result
    }

    fn execute_blocks(
        &mut self,
        results: &[TypeId],
        blocks: &[pop_mir::MirBlock],
        arguments: &[RuntimeValue],
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let mut values = BTreeMap::new();
        let entry = blocks.first().ok_or(ExecutionError::InvalidControlFlow)?;
        for (argument, value) in entry.arguments().iter().zip(arguments) {
            values.insert(argument.value(), value.clone());
        }
        let mut block_index = 0_usize;
        let mut pending_unwind = None;
        loop {
            self.step()?;
            let block = blocks
                .get(block_index)
                .ok_or(ExecutionError::InvalidControlFlow)?;
            let mut unwound_to_cleanup = None;
            for instruction in block.instructions() {
                self.step()?;
                let evaluated = if instruction.has_result() {
                    self.evaluate_instruction(instruction, &mut values)
                        .map(Some)
                } else {
                    self.evaluate_effect_instruction(instruction, &mut values)
                        .map(|()| None)
                };
                match evaluated {
                    Ok(Some(value)) => {
                        values.insert(instruction.result(), value);
                    }
                    Ok(None) => {}
                    Err(ExecutionError::Runtime(RuntimeFailure::Unwind(reason))) => {
                        if pending_unwind.is_some() {
                            return Err(ExecutionError::Runtime(self.runtime.begin_panic(
                                pop_runtime_interface::PanicPayload::new(
                                    pop_runtime_interface::PanicKind::DoublePanic,
                                ),
                            )));
                        }
                        if let Some(target) = call_cleanup_target(instruction) {
                            pending_unwind = Some(reason);
                            unwound_to_cleanup = Some(target.raw() as usize);
                            break;
                        }
                        return Err(ExecutionError::Runtime(RuntimeFailure::Unwind(reason)));
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some(cleanup) = unwound_to_cleanup {
                block_index = cleanup;
                continue;
            }
            self.step()?;
            match block.terminator() {
                MirTerminator::Branch { target, arguments } => {
                    Self::assign_block_arguments(blocks, *target, arguments, &mut values)?;
                    block_index = target.raw() as usize;
                }
                MirTerminator::ConditionalBranch {
                    condition,
                    when_true,
                    when_false,
                } => {
                    let target = match &value(&values, *condition)?.visible {
                        MirValue::Boolean(true) => *when_true,
                        MirValue::Boolean(false) => *when_false,
                        _ => return Err(ExecutionError::TypeMismatch),
                    };
                    block_index = target.raw() as usize;
                }
                MirTerminator::UnionSwitch {
                    scrutinee,
                    union,
                    arms,
                } => {
                    let MirValue::Union {
                        union: value_union,
                        case,
                        arguments,
                    } = value(&values, *scrutinee)?.visible.clone()
                    else {
                        return Err(ExecutionError::TypeMismatch);
                    };
                    if value_union != *union {
                        return Err(ExecutionError::TypeMismatch);
                    }
                    let arm = arms
                        .iter()
                        .find(|arm| arm.case() == case)
                        .ok_or(ExecutionError::InvalidControlFlow)?;
                    Self::assign_runtime_block_arguments(
                        blocks,
                        arm.target(),
                        &arguments,
                        &mut values,
                    )?;
                    block_index = arm.target().raw() as usize;
                }
                MirTerminator::ErrorSwitch {
                    scrutinee,
                    error,
                    arms,
                } => {
                    let MirValue::Error {
                        error: value_error,
                        case,
                        arguments,
                    } = value(&values, *scrutinee)?.visible.clone()
                    else {
                        return Err(ExecutionError::TypeMismatch);
                    };
                    if value_error != *error {
                        return Err(ExecutionError::TypeMismatch);
                    }
                    let arm = arms
                        .iter()
                        .find(|arm| arm.case() == case)
                        .ok_or(ExecutionError::InvalidControlFlow)?;
                    Self::assign_runtime_block_arguments(
                        blocks,
                        arm.target(),
                        &arguments,
                        &mut values,
                    )?;
                    block_index = arm.target().raw() as usize;
                }
                MirTerminator::CodecErrorSwitch { scrutinee, arms } => {
                    let MirValue::CodecError(error) = &value(&values, *scrutinee)?.visible else {
                        return Err(ExecutionError::TypeMismatch);
                    };
                    let arm = arms
                        .iter()
                        .find(|arm| arm.case() == error.case())
                        .ok_or(ExecutionError::InvalidControlFlow)?;
                    Self::assign_runtime_block_arguments(blocks, arm.target(), &[], &mut values)?;
                    block_index = arm.target().raw() as usize;
                }
                MirTerminator::Return { values: returned } => {
                    let returned: Vec<_> = returned
                        .iter()
                        .map(|value_id| value(&values, *value_id).cloned())
                        .collect::<Result<_, _>>()?;
                    require_runtime_numeric_types(self.arena, results, &returned)?;
                    return Ok(returned);
                }
                MirTerminator::Trap(trap) => {
                    return Err(ExecutionError::Runtime(self.runtime.raise_trap(*trap)));
                }
                MirTerminator::Panic(payload) => {
                    if pending_unwind.is_some() {
                        return Err(ExecutionError::Runtime(self.runtime.begin_panic(
                            pop_runtime_interface::PanicPayload::new(
                                pop_runtime_interface::PanicKind::DoublePanic,
                            ),
                        )));
                    }
                    return Err(ExecutionError::Runtime(
                        self.runtime.begin_panic(payload.clone()),
                    ));
                }
                MirTerminator::ContinueUnwind(reason) => {
                    if pending_unwind.is_some() {
                        return Err(ExecutionError::Runtime(self.runtime.begin_panic(
                            pop_runtime_interface::PanicPayload::new(
                                pop_runtime_interface::PanicKind::DoublePanic,
                            ),
                        )));
                    }
                    return Err(ExecutionError::Runtime(RuntimeFailure::Unwind(
                        reason.clone(),
                    )));
                }
                MirTerminator::ResumeUnwind => {
                    let reason = pending_unwind
                        .take()
                        .ok_or(ExecutionError::InvalidControlFlow)?;
                    return Err(ExecutionError::Runtime(RuntimeFailure::Unwind(reason)));
                }
                MirTerminator::Suspend {
                    operation: MirSuspendOperation::Task { task, result_type },
                    resume,
                    cancellation,
                    cancellation_mode,
                    unwind,
                    live_frame,
                    ..
                } => {
                    if *cancellation_mode == MirCancellationMode::Observe
                        && self.active_cancellation_observation(false)
                            == CancellationObservation::Requested
                    {
                        pending_unwind = None;
                        block_index = cancellation.raw() as usize;
                        continue;
                    }
                    self.publish_suspend_frame(live_frame, &mut values)?;
                    let task = value(&values, *task)?.clone();
                    match self.await_task(&task, *result_type) {
                        Ok(completion) => {
                            let resume_block = blocks
                                .get(resume.raw() as usize)
                                .ok_or(ExecutionError::InvalidControlFlow)?;
                            let [argument] = resume_block.arguments() else {
                                return Err(ExecutionError::WrongArity);
                            };
                            values.insert(argument.value(), completion);
                            block_index = resume.raw() as usize;
                        }
                        Err(ExecutionError::Runtime(RuntimeFailure::Unwind(
                            pop_runtime_interface::UnwindReason::Cancellation,
                        ))) => {
                            pending_unwind = None;
                            block_index = cancellation.raw() as usize;
                        }
                        Err(ExecutionError::Runtime(RuntimeFailure::Unwind(reason))) => {
                            if let MirUnwindAction::Cleanup(target) = unwind {
                                pending_unwind = Some(reason);
                                block_index = target.raw() as usize;
                            } else {
                                return Err(ExecutionError::Runtime(RuntimeFailure::Unwind(
                                    reason,
                                )));
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
                MirTerminator::Unreachable => return Err(ExecutionError::ReachedUnreachable),
                MirTerminator::Missing => return Err(ExecutionError::InvalidControlFlow),
            }
        }
    }

    fn active_cancellation_observation(&self, masked: bool) -> CancellationObservation {
        let Some(task) = self.active_task else {
            return CancellationObservation::Active;
        };
        self.private_values
            .values()
            .find_map(|value| match value {
                PrivateValue::Task(state) if state.borrow().lifecycle.id() == task => {
                    Some(state.borrow().lifecycle.cancellation_observation(masked))
                }
                _ => None,
            })
            .unwrap_or(CancellationObservation::Active)
    }

    fn publish_suspend_frame(
        &mut self,
        frame: &pop_mir::MirLiveFrame,
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<(), ExecutionError> {
        let roots = frame
            .stack_map()
            .root_slots()
            .iter()
            .map(|root| {
                frame
                    .slots()
                    .get(root.raw() as usize)
                    .ok_or(ExecutionError::InvalidControlFlow)
                    .and_then(|slot| value(values, slot.value()).map(|value| value.reference))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut publication = RootPublication::new(frame.stack_map().clone(), roots)
            .map_err(|_| ExecutionError::InvalidControlFlow)?;
        self.runtime
            .safe_point(&mut publication)
            .map_err(ExecutionError::Runtime)?;
        let root_values = frame
            .stack_map()
            .root_slots()
            .iter()
            .map(|root| {
                frame
                    .slots()
                    .get(root.raw() as usize)
                    .map(|slot| slot.value())
                    .ok_or(ExecutionError::InvalidControlFlow)
            })
            .collect::<Result<Vec<_>, _>>()?;
        install_published_relocations(&root_values, &publication, values)?;
        Ok(())
    }

    fn await_task(
        &mut self,
        task: &RuntimeValue,
        expected_completion_type: TypeId,
    ) -> Result<RuntimeValue, ExecutionError> {
        let MirValue::Task(task) = &task.visible else {
            return Err(ExecutionError::TypeMismatch);
        };
        let state = match self.private_values.get(task) {
            Some(PrivateValue::Task(state)) => state.clone(),
            _ => return Err(ExecutionError::InvalidControlFlow),
        };
        let (target, arguments, completion_type, owner, completion_slot) = {
            let mut state = state.borrow_mut();
            let completion_type = state.completion_type;
            match state.execution.clone() {
                TaskExecution::Completed(result) => return result,
                TaskExecution::Running => return Err(ExecutionError::InvalidControlFlow),
                TaskExecution::Created {
                    target,
                    arguments,
                    owner,
                    completion_slot,
                } => {
                    let created = (target, arguments, completion_type, owner, completion_slot);
                    if state.lifecycle.state() == RuntimeTaskState::Created {
                        state
                            .lifecycle
                            .start(TaskOwner::DirectAwait {
                                parent: self.active_task,
                            })
                            .map_err(|_| ExecutionError::InvalidControlFlow)?;
                    } else if !matches!(state.lifecycle.owner(), Some(TaskOwner::Group(_))) {
                        return Err(ExecutionError::InvalidControlFlow);
                    }
                    state
                        .lifecycle
                        .begin_poll()
                        .map_err(|_| ExecutionError::InvalidControlFlow)?;
                    state.execution = TaskExecution::Running;
                    created
                }
            }
        };
        if completion_type != expected_completion_type {
            let result = Err(ExecutionError::TypeMismatch);
            let mut state = state.borrow_mut();
            state
                .lifecycle
                .finish_poll(TaskPollCompletion::Panicked)
                .map_err(|_| ExecutionError::InvalidControlFlow)?;
            state.execution = TaskExecution::Completed(result.clone());
            return result;
        }
        let active_task = state.borrow().lifecycle.id();
        let previous_active_task = self.active_task.replace(active_task);
        let mut result = match target {
            TaskTarget::Direct(function) => self.call(function, &arguments),
            TaskTarget::Referenced(function) => {
                Err(ExecutionError::UnknownReferencedFunction(function))
            }
            TaskTarget::Indirect(callee) => self.execute_indirect_value(&callee, &arguments),
            TaskTarget::Group { body, group } => self
                .execute_task_group(&body, group, completion_type)
                .map(|completion| vec![completion]),
        }
        .and_then(|returned| self.task_completion(completion_type, returned));
        self.active_task = previous_active_task;
        if let Ok(completion) = &result
            && let Some(reference) = completion.reference
            && let Err(failure) = self.runtime.write_barrier(WriteBarrier::new(
                BarrierKind::CombinedSatbGenerational,
                owner,
                completion_slot,
                None,
                Some(reference),
            ))
        {
            result = Err(ExecutionError::Runtime(failure));
        }
        let completion = match &result {
            Ok(_) => TaskPollCompletion::Completed,
            Err(ExecutionError::Runtime(RuntimeFailure::Unwind(
                pop_runtime_interface::UnwindReason::Cancellation,
            ))) => TaskPollCompletion::Cancelled,
            Err(_) => TaskPollCompletion::Panicked,
        };
        let mut state = state.borrow_mut();
        state
            .lifecycle
            .finish_poll(completion)
            .map_err(|_| ExecutionError::InvalidControlFlow)?;
        debug_assert!(matches!(
            state.lifecycle.state(),
            RuntimeTaskState::Completed | RuntimeTaskState::Cancelled | RuntimeTaskState::Panicked
        ));
        state.execution = TaskExecution::Completed(result.clone());
        result
    }

    fn execute_task_group(
        &mut self,
        body: &RuntimeValue,
        group_symbol: SymbolId,
        completion_type: TypeId,
    ) -> Result<RuntimeValue, ExecutionError> {
        let group = match self.private_values.get(&group_symbol) {
            Some(PrivateValue::TaskGroup(group)) => group.clone(),
            _ => return Err(ExecutionError::InvalidControlFlow),
        };
        let group_value = {
            let group = group.borrow();
            RuntimeValue::managed(MirValue::TaskGroup(group_symbol), group.reference)
        };
        let body_result = self
            .execute_indirect_value(body, &[group_value])
            .and_then(|returned| self.task_completion(completion_type, returned));
        let exit = match &body_result {
            Ok(_) => TaskGroupExit::BodyCompleted,
            Err(ExecutionError::Runtime(RuntimeFailure::Unwind(UnwindReason::Cancellation))) => {
                TaskGroupExit::Cancelled
            }
            Err(ExecutionError::Runtime(RuntimeFailure::Unwind(UnwindReason::Panic(_)))) => {
                TaskGroupExit::BodyPanicked
            }
            Err(_) => TaskGroupExit::BodyFailed,
        };
        let children = group
            .borrow_mut()
            .lifecycle
            .begin_close(exit)
            .map_err(|_| ExecutionError::InvalidControlFlow)?;
        let mut child_failure = None;
        for child_id in children {
            let child_symbol = group
                .borrow()
                .children
                .get(&child_id)
                .copied()
                .ok_or(ExecutionError::InvalidControlFlow)?;
            let child_state = match self.private_values.get(&child_symbol) {
                Some(PrivateValue::Task(child)) => child.clone(),
                _ => return Err(ExecutionError::InvalidControlFlow),
            };
            let (completion_type, child_value) = {
                let mut child = child_state.borrow_mut();
                let token = group.borrow().lifecycle.cancellation_token();
                if !child.lifecycle.state().terminal() {
                    let _ = child.lifecycle.request_cancellation(token);
                }
                let reference = match &child.execution {
                    TaskExecution::Created { owner, .. } => *owner,
                    TaskExecution::Running | TaskExecution::Completed(_) => {
                        group.borrow().reference
                    }
                };
                (
                    child.completion_type,
                    RuntimeValue::managed(MirValue::Task(child_symbol), reference),
                )
            };
            let outcome = self.await_task(&child_value, completion_type);
            if child_failure.is_none() {
                child_failure = outcome.err();
            }
            group
                .borrow_mut()
                .lifecycle
                .join_child(&child_state.borrow().lifecycle)
                .map_err(|_| ExecutionError::InvalidControlFlow)?;
        }
        group
            .borrow_mut()
            .lifecycle
            .complete_close()
            .map_err(|_| ExecutionError::InvalidControlFlow)?;
        match body_result {
            Err(error) => Err(error),
            Ok(_) if child_failure.is_some() => Err(child_failure.expect("checked child failure")),
            Ok(completion) => Ok(completion),
        }
    }

    fn task_completion(
        &mut self,
        result_type: TypeId,
        mut returned: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, ExecutionError> {
        if returned.len() == 1 {
            return Ok(returned.remove(0));
        }
        let reference_slots = returned
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                value
                    .reference
                    .map(|_| ObjectSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
            })
            .collect();
        let object_map = ObjectMap::new(
            u32::try_from(returned.len()).unwrap_or(u32::MAX),
            reference_slots,
        )
        .map_err(|_| ExecutionError::InvalidControlFlow)?;
        let reference = self
            .runtime
            .allocate_object(&ObjectAllocationRequest::new(
                RuntimeTypeId::new(result_type.raw()),
                AllocationClass::NurseryEligible,
                object_map,
            ))
            .map_err(ExecutionError::Runtime)?;
        Ok(RuntimeValue::managed(
            MirValue::Tuple(returned.into_iter().map(|value| value.visible).collect()),
            reference,
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn evaluate_instruction(
        &mut self,
        instruction: &MirInstruction,
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<RuntimeValue, ExecutionError> {
        if let Some(result) = self.evaluate_structured_instruction(instruction, values)? {
            return Ok(result);
        }
        match evaluate_numeric_instruction(instruction.kind(), values) {
            Ok(Some(result)) => return Ok(RuntimeValue::visible(result)),
            Ok(None) => {}
            Err(ExecutionError::IntegerOverflow) => {
                return Err(ExecutionError::Runtime(
                    self.runtime
                        .raise_trap(Trap::new(TrapKind::IntegerOverflow)),
                ));
            }
            Err(ExecutionError::DivisionByZero) => {
                return Err(ExecutionError::Runtime(
                    self.runtime.raise_trap(Trap::new(TrapKind::DivisionByZero)),
                ));
            }
            Err(ExecutionError::NumericConversion) => {
                return Err(ExecutionError::Runtime(
                    self.runtime
                        .raise_trap(Trap::new(TrapKind::NumericConversion)),
                ));
            }
            Err(error) => return Err(error),
        }
        let result = match instruction.kind() {
            MirInstructionKind::TaskCreate {
                dispatch,
                arguments,
                completion_type,
                object_map,
            } => {
                let arguments = evaluated_arguments(arguments, values)?;
                let target = match dispatch {
                    MirTaskDispatch::Direct(function) => TaskTarget::Direct(*function),
                    MirTaskDispatch::Referenced(function) => TaskTarget::Referenced(*function),
                    MirTaskDispatch::Indirect(callee) => {
                        let callee = value(values, *callee)?.clone();
                        if !matches!(callee.visible, MirValue::Function(_)) {
                            return Err(ExecutionError::TypeMismatch);
                        }
                        TaskTarget::Indirect(callee)
                    }
                };
                let mut stored = arguments.clone();
                if let TaskTarget::Indirect(callee) = &target {
                    stored.insert(0, callee.clone());
                }
                if stored.iter().enumerate().any(|(index, value)| {
                    value.reference.is_some()
                        && !object_map.is_reference_slot(ObjectSlot::new(
                            u32::try_from(index).unwrap_or(u32::MAX),
                        ))
                }) {
                    return Err(ExecutionError::InvalidControlFlow);
                }
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        object_map.clone(),
                    ))
                    .map_err(ExecutionError::Runtime)?;
                let completion_slot = object_map
                    .slot_count()
                    .checked_sub(1)
                    .map(ObjectSlot::new)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let task = self.fresh_private_symbol();
                self.private_values.insert(
                    task,
                    PrivateValue::Task(Rc::new(RefCell::new(TaskState {
                        lifecycle: TaskLifecycle::created(TaskId::new(u64::from(task.raw()))),
                        completion_type: *completion_type,
                        execution: TaskExecution::Created {
                            target,
                            arguments,
                            owner: reference,
                            completion_slot,
                        },
                    }))),
                );
                return Ok(RuntimeValue::managed(MirValue::Task(task), reference));
            }
            MirInstructionKind::CancelSourceCreate => {
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        ObjectMap::new(0, Vec::new())
                            .map_err(|_| ExecutionError::InvalidControlFlow)?,
                    ))
                    .map_err(ExecutionError::Runtime)?;
                let source = self.fresh_private_symbol();
                let cancellation = Rc::new(RefCell::new(CancellationState {
                    token: CancellationTokenId::new(u64::from(source.raw())),
                    requested: false,
                }));
                self.private_values
                    .insert(source, PrivateValue::CancellationSource(cancellation));
                return Ok(RuntimeValue::managed(
                    MirValue::CancellationSource(source),
                    reference,
                ));
            }
            MirInstructionKind::CancelSourceToken { source } => {
                let source = value(values, *source)?.clone();
                let MirValue::CancellationSource(source_symbol) = source.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let cancellation = match self.private_values.get(&source_symbol) {
                    Some(PrivateValue::CancellationSource(cancellation)) => cancellation.clone(),
                    _ => return Err(ExecutionError::InvalidControlFlow),
                };
                let token = self.fresh_private_symbol();
                self.private_values
                    .insert(token, PrivateValue::CancellationToken(cancellation));
                return Ok(RuntimeValue {
                    visible: MirValue::CancellationToken(token),
                    reference: source.reference,
                });
            }
            MirInstructionKind::CancelRequest { source } => {
                let MirValue::CancellationSource(source) = value(values, *source)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let cancellation = match self.private_values.get(&source) {
                    Some(PrivateValue::CancellationSource(cancellation)) => cancellation.clone(),
                    _ => return Err(ExecutionError::InvalidControlFlow),
                };
                let token = {
                    let mut cancellation = cancellation.borrow_mut();
                    cancellation.requested = true;
                    cancellation.token
                };
                let tasks = self
                    .private_values
                    .values()
                    .filter_map(|value| match value {
                        PrivateValue::Task(task) => Some(task.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                for task in tasks {
                    let mut task = task.borrow_mut();
                    if task.lifecycle.cancellation_token() == Some(token)
                        && !task.lifecycle.state().terminal()
                    {
                        let _ = task.lifecycle.request_cancellation(token);
                    }
                }
                MirValue::Nil
            }
            MirInstructionKind::TaskGroupCreate {
                cancel,
                body,
                completion_type,
                object_map,
            } => {
                let cancel = value(values, *cancel)?.clone();
                let body = value(values, *body)?.clone();
                let MirValue::CancellationToken(token_symbol) = cancel.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if !matches!(body.visible, MirValue::Function(_)) {
                    return Err(ExecutionError::TypeMismatch);
                }
                let cancellation = match self.private_values.get(&token_symbol) {
                    Some(PrivateValue::CancellationToken(cancellation)) => cancellation.clone(),
                    _ => return Err(ExecutionError::InvalidControlFlow),
                };
                for (index, stored) in [&cancel, &body].into_iter().enumerate() {
                    if stored.reference.is_some()
                        && !object_map.is_reference_slot(ObjectSlot::new(
                            u32::try_from(index).unwrap_or(u32::MAX),
                        ))
                    {
                        return Err(ExecutionError::InvalidControlFlow);
                    }
                }
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        object_map.clone(),
                    ))
                    .map_err(ExecutionError::Runtime)?;
                let group_symbol = self.fresh_private_symbol();
                let group_id = TaskGroupId::new(u64::from(group_symbol.raw()));
                let token = cancellation.borrow().token;
                self.private_values.insert(
                    group_symbol,
                    PrivateValue::TaskGroup(Rc::new(RefCell::new(InterpreterTaskGroup {
                        lifecycle: TaskGroupLifecycle::open(group_id, token),
                        cancellation: cancellation.clone(),
                        children: BTreeMap::new(),
                        reference,
                    }))),
                );
                let task_symbol = self.fresh_private_symbol();
                let mut lifecycle =
                    TaskLifecycle::created(TaskId::new(u64::from(task_symbol.raw())));
                lifecycle
                    .bind_cancellation_token(token)
                    .map_err(|_| ExecutionError::InvalidControlFlow)?;
                if cancellation.borrow().requested {
                    lifecycle
                        .request_cancellation(token)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?;
                }
                let completion_slot = object_map
                    .slot_count()
                    .checked_sub(1)
                    .map(ObjectSlot::new)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.private_values.insert(
                    task_symbol,
                    PrivateValue::Task(Rc::new(RefCell::new(TaskState {
                        lifecycle,
                        completion_type: *completion_type,
                        execution: TaskExecution::Created {
                            target: TaskTarget::Group {
                                body,
                                group: group_symbol,
                            },
                            arguments: Vec::new(),
                            owner: reference,
                            completion_slot,
                        },
                    }))),
                );
                return Ok(RuntimeValue::managed(
                    MirValue::Task(task_symbol),
                    reference,
                ));
            }
            MirInstructionKind::TaskStart { group, task } => {
                let task_value = value(values, *task)?.clone();
                let MirValue::TaskGroup(group_symbol) = value(values, *group)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let MirValue::Task(task_symbol) = task_value.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let group = match self.private_values.get(&group_symbol) {
                    Some(PrivateValue::TaskGroup(group)) => group.clone(),
                    _ => return Err(ExecutionError::InvalidControlFlow),
                };
                let task = match self.private_values.get(&task_symbol) {
                    Some(PrivateValue::Task(task)) => task.clone(),
                    _ => return Err(ExecutionError::InvalidControlFlow),
                };
                {
                    let mut group = group.borrow_mut();
                    let mut task = task.borrow_mut();
                    group
                        .lifecycle
                        .start_child(&mut task.lifecycle)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?;
                    group.children.insert(task.lifecycle.id(), task_symbol);
                    if group.cancellation.borrow().requested {
                        let token = group.lifecycle.cancellation_token();
                        task.lifecycle
                            .request_cancellation(token)
                            .map_err(|_| ExecutionError::InvalidControlFlow)?;
                    }
                }
                return Ok(task_value);
            }
            MirInstructionKind::StringConstant(value) => MirValue::String(value.clone()),
            MirInstructionKind::StringConcat { left, right } => {
                let MirValue::String(left) = &value(values, *left)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let MirValue::String(right) = &value(values, *right)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let mut result = String::with_capacity(left.len().saturating_add(right.len()));
                result.push_str(left);
                result.push_str(right);
                MirValue::String(result)
            }
            MirInstructionKind::ViewCreate { kind, lender, .. } => {
                let (lender, byte_length, scalar_length) =
                    match (kind, &value(values, *lender)?.visible) {
                        (pop_mir::MirViewKind::Bytes, MirValue::Bytes(reference)) => {
                            let length = self
                                .runtime
                                .immutable_bytes_length(*reference)
                                .map_err(ExecutionError::Runtime)?;
                            let length = usize::try_from(length)
                                .map_err(|_| ExecutionError::InvalidControlFlow)?;
                            (MirViewLenderValue::Bytes(*reference), length, length)
                        }
                        (pop_mir::MirViewKind::Text, MirValue::String(text)) => (
                            MirViewLenderValue::Text(Rc::from(text.as_str())),
                            text.len(),
                            text.chars().count(),
                        ),
                        _ => return Err(ExecutionError::TypeMismatch),
                    };
                MirValue::View(MirViewValue {
                    kind: *kind,
                    lender,
                    byte_offset: 0,
                    byte_length,
                    scalar_length,
                })
            }
            MirInstructionKind::ViewSlice {
                kind,
                view,
                start,
                length,
                ..
            } => {
                let MirValue::View(parent) = &value(values, *view)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if parent.kind != *kind {
                    return Err(ExecutionError::TypeMismatch);
                }
                let start = integer_i64(&value(values, *start)?.visible)?;
                let length = integer_i64(&value(values, *length)?.visible)?;
                let owner_length = match kind {
                    pop_mir::MirViewKind::Bytes => parent.byte_length,
                    pop_mir::MirViewKind::Text => parent.scalar_length,
                };
                let (relative_start, selected_length) =
                    checked_view_range(owner_length, start, length)
                        .ok_or_else(|| self.bounds_violation())?;
                let (byte_start, byte_length) = match kind {
                    pop_mir::MirViewKind::Bytes => (relative_start, selected_length),
                    pop_mir::MirViewKind::Text => {
                        let text = view_text(parent)?;
                        let start = scalar_byte_offset(text, relative_start)
                            .ok_or(ExecutionError::InvalidControlFlow)?;
                        let end = scalar_byte_offset(
                            text,
                            relative_start.saturating_add(selected_length),
                        )
                        .ok_or(ExecutionError::InvalidControlFlow)?;
                        (start, end - start)
                    }
                };
                MirValue::View(MirViewValue {
                    kind: *kind,
                    lender: parent.lender.clone(),
                    byte_offset: parent
                        .byte_offset
                        .checked_add(byte_start)
                        .ok_or_else(|| self.integer_overflow())?,
                    byte_length,
                    scalar_length: match kind {
                        pop_mir::MirViewKind::Bytes => byte_length,
                        pop_mir::MirViewKind::Text => selected_length,
                    },
                })
            }
            MirInstructionKind::ViewLength { kind, view } => {
                let MirValue::View(view) = &value(values, *view)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if view.kind != *kind {
                    return Err(ExecutionError::TypeMismatch);
                }
                let length = match kind {
                    pop_mir::MirViewKind::Bytes => view.byte_length,
                    pop_mir::MirViewKind::Text => view.scalar_length,
                };
                MirValue::Integer(
                    IntegerValue::parse_decimal(&length.to_string(), IntegerKind::Int64)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                )
            }
            MirInstructionKind::ViewGetByte { view, index } => {
                let MirValue::View(view) = &value(values, *view)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if view.kind != pop_mir::MirViewKind::Bytes {
                    return Err(ExecutionError::TypeMismatch);
                }
                let index = integer_i64(&value(values, *index)?.visible)?;
                let Some(relative) = index
                    .checked_sub(1)
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|index| *index < view.byte_length)
                else {
                    return Ok(RuntimeValue::visible(MirValue::Nil));
                };
                let reference = view_bytes_reference(view)?;
                let mut byte = [0_u8; 1];
                let offset = view
                    .byte_offset
                    .checked_add(relative)
                    .and_then(|offset| u64::try_from(offset).ok())
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.runtime
                    .immutable_bytes_read(reference, offset, &mut byte)
                    .map_err(ExecutionError::Runtime)?;
                MirValue::Integer(
                    IntegerValue::parse_decimal(&byte[0].to_string(), IntegerKind::UInt8)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                )
            }
            MirInstructionKind::ViewMaterialize { kind, view, .. } => {
                let MirValue::View(view) = &value(values, *view)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if view.kind != *kind {
                    return Err(ExecutionError::TypeMismatch);
                }
                match kind {
                    pop_mir::MirViewKind::Text => MirValue::String(view_text(view)?.to_owned()),
                    pop_mir::MirViewKind::Bytes => {
                        let reference = view_bytes_reference(view)?;
                        let mut bytes = vec![0_u8; view.byte_length];
                        self.runtime
                            .immutable_bytes_read(
                                reference,
                                u64::try_from(view.byte_offset)
                                    .map_err(|_| ExecutionError::InvalidControlFlow)?,
                                &mut bytes,
                            )
                            .map_err(ExecutionError::Runtime)?;
                        let reference = self
                            .runtime
                            .allocate_immutable_bytes(&bytes)
                            .map_err(ExecutionError::Runtime)?;
                        MirValue::Bytes(reference)
                    }
                }
            }
            MirInstructionKind::StringFormat {
                kind,
                value: operand,
            } => {
                let operand = &value(values, *operand)?.visible;
                let formatted = match (kind, operand) {
                    (pop_types::StringFormatKind::Boolean, MirValue::Boolean(value)) => {
                        value.to_string()
                    }
                    (pop_types::StringFormatKind::Integer(expected), MirValue::Integer(value))
                        if expected == &value.kind() =>
                    {
                        value.to_string()
                    }
                    (pop_types::StringFormatKind::Float(expected), MirValue::Float(value))
                        if expected == &value.kind() =>
                    {
                        value.format_string()
                    }
                    _ => return Err(ExecutionError::TypeMismatch),
                };
                MirValue::String(formatted)
            }
            MirInstructionKind::BooleanConstant(value) => MirValue::Boolean(*value),
            MirInstructionKind::NilConstant => MirValue::Nil,
            MirInstructionKind::FfiPointerNone => MirValue::Nil,
            MirInstructionKind::FfiPointerToOptional { pointer }
            | MirInstructionKind::FfiPointerReadOnly { pointer } => {
                let pointer = value(values, *pointer)?.visible.clone();
                if !matches!(pointer, MirValue::FfiPointer(_)) {
                    return Err(ExecutionError::TypeMismatch);
                }
                pointer
            }
            MirInstructionKind::FfiPointerIsPresent { pointer } => {
                match &value(values, *pointer)?.visible {
                    MirValue::Nil => MirValue::Boolean(false),
                    MirValue::FfiPointer(_) => MirValue::Boolean(true),
                    _ => return Err(ExecutionError::TypeMismatch),
                }
            }
            MirInstructionKind::FfiPointerRequire {
                pointer,
                result,
                success,
                failure,
            } => {
                let (case, arguments) = match &value(values, *pointer)?.visible {
                    MirValue::FfiPointer(address) => {
                        (*success, vec![MirValue::FfiPointer(*address)])
                    }
                    MirValue::Nil => (*failure, vec![MirValue::FfiNullPointerError]),
                    _ => return Err(ExecutionError::TypeMismatch),
                };
                MirValue::Result {
                    definition: *result,
                    case,
                    arguments,
                }
            }
            MirInstructionKind::OptionalIsPresent { optional } => {
                MirValue::Boolean(!matches!(value(values, *optional)?.visible, MirValue::Nil))
            }
            MirInstructionKind::OptionalGet { optional } => {
                let present = value(values, *optional)?.visible.clone();
                if matches!(present, MirValue::Nil) {
                    return Err(ExecutionError::InvalidControlFlow);
                }
                present
            }
            MirInstructionKind::ResultIsOk { result, definition } => {
                let MirValue::Result {
                    definition: found,
                    case,
                    ..
                } = &value(values, *result)?.visible
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if found != definition {
                    return Err(ExecutionError::TypeMismatch);
                }
                MirValue::Boolean(case.raw() == 0)
            }
            MirInstructionKind::ResultGetOk { result, definition }
            | MirInstructionKind::ResultGetError { result, definition } => {
                let MirValue::Result {
                    definition: found,
                    case,
                    arguments,
                } = &value(values, *result)?.visible
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let expected = u32::from(matches!(
                    instruction.kind(),
                    MirInstructionKind::ResultGetError { .. }
                ));
                if found != definition || case.raw() != expected || arguments.len() != 1 {
                    return Err(ExecutionError::InvalidControlFlow);
                }
                arguments[0].clone()
            }
            MirInstructionKind::IterationIsItem {
                iteration,
                definition,
                item_case,
                end_case,
            } => {
                let MirValue::Iteration {
                    definition: found,
                    case,
                    ..
                } = &value(values, *iteration)?.visible
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if found != definition || (case != item_case && case != end_case) {
                    return Err(ExecutionError::InvalidControlFlow);
                }
                MirValue::Boolean(case == item_case)
            }
            MirInstructionKind::IterationGetItem {
                iteration,
                definition,
                item_case,
            } => {
                let MirValue::Iteration {
                    definition: found,
                    case,
                    arguments,
                } = &value(values, *iteration)?.visible
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if found != definition || case != item_case || arguments.len() != 1 {
                    return Err(ExecutionError::InvalidControlFlow);
                }
                arguments[0].clone()
            }
            MirInstructionKind::EnumConstant {
                definition,
                case,
                discriminant,
            } => MirValue::Enum {
                definition: *definition,
                case: *case,
                discriminant: *discriminant,
            },
            MirInstructionKind::CodecErrorConstant { case } => {
                let reason = match case.raw() {
                    0 => MirCodecError::MalformedInput,
                    1 => MirCodecError::LimitExceeded,
                    2 => MirCodecError::CapabilityFailure,
                    _ => return Err(ExecutionError::InvalidControlFlow),
                };
                MirValue::CodecError(reason)
            }
            MirInstructionKind::FunctionReference(function) => MirValue::Function(*function),
            MirInstructionKind::GeneratedCodecSchema(adapter) => MirValue::CodecSchema(*adapter),
            MirInstructionKind::CodecEncode {
                adapter,
                value: input,
                writer,
                result,
                success,
                failure,
            } => {
                let catalog = self.mir.generated_codec_adapters();
                let adapter = catalog
                    .iter()
                    .find(|candidate| candidate.symbol() == *adapter)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let input = &value(values, *input)?.visible;
                let MirValue::CodecWriter(writer) = &value(values, *writer)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let mut events = Vec::new();
                let encoded = encode_codec_value(
                    adapter,
                    input,
                    &mut events,
                    self.arena,
                    catalog,
                    self.runtime,
                    0,
                );
                let committed = encoded.is_ok()
                    && !events.is_empty()
                    && writer.append_within_limit(events, MAX_CODEC_EVENTS);
                match encoded {
                    Ok(()) if committed => MirValue::Result {
                        definition: *result,
                        case: *success,
                        arguments: vec![MirValue::Nil],
                    },
                    Ok(()) | Err(MirCodecError::LimitExceeded) => MirValue::Result {
                        definition: *result,
                        case: *failure,
                        arguments: vec![MirValue::CodecError(MirCodecError::LimitExceeded)],
                    },
                    Err(error) => MirValue::Result {
                        definition: *result,
                        case: *failure,
                        arguments: vec![MirValue::CodecError(error)],
                    },
                }
            }
            MirInstructionKind::CodecDecode {
                adapter,
                reader,
                result,
                success,
                failure,
            } => {
                let catalog = self.mir.generated_codec_adapters();
                let adapter = catalog
                    .iter()
                    .find(|candidate| candidate.symbol() == *adapter)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let MirValue::CodecReader(reader) = &value(values, *reader)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let decoded = if reader.events.len() > MAX_CODEC_EVENTS {
                    Err(MirCodecError::LimitExceeded)
                } else {
                    decode_codec_value(adapter, reader, self.arena, catalog, self.runtime, 0)
                };
                match decoded {
                    Ok(decoded) => MirValue::Result {
                        definition: *result,
                        case: *success,
                        arguments: vec![decoded],
                    },
                    Err(MirCodecError::MalformedInput) => MirValue::Result {
                        definition: *result,
                        case: *failure,
                        arguments: vec![MirValue::CodecError(MirCodecError::MalformedInput)],
                    },
                    Err(error) => MirValue::Result {
                        definition: *result,
                        case: *failure,
                        arguments: vec![MirValue::CodecError(error)],
                    },
                }
            }
            MirInstructionKind::TupleMake(elements) => {
                let tuple = MirValue::Tuple(
                    elements
                        .iter()
                        .map(|element| value(values, *element).map(|value| value.visible.clone()))
                        .collect::<Result<_, _>>()?,
                );
                let Some(SemanticType::Tuple(element_types)) =
                    self.arena.get(instruction.result_type())
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let references = element_types
                    .iter()
                    .enumerate()
                    .filter_map(|(index, type_id)| {
                        managed_type(self.arena, *type_id)
                            .then(|| u32::try_from(index).ok().map(ObjectSlot::new))
                            .flatten()
                    })
                    .collect();
                let object_map = ObjectMap::new(
                    u32::try_from(element_types.len()).unwrap_or(u32::MAX),
                    references,
                )
                .map_err(|_| ExecutionError::InvalidControlFlow)?;
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        object_map,
                    ))
                    .map_err(ExecutionError::Runtime)?;
                return Ok(RuntimeValue::managed(tuple, reference));
            }
            MirInstructionKind::TupleGet { tuple, index } => {
                let MirValue::Tuple(elements) = &value(values, *tuple)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                elements
                    .get(*index as usize)
                    .cloned()
                    .ok_or(ExecutionError::InvalidControlFlow)?
            }
            MirInstructionKind::ArrayMake {
                elements,
                element_map,
            } => {
                let reference = self
                    .runtime
                    .allocate_array(&ArrayAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        u32::try_from(elements.len()).unwrap_or(u32::MAX),
                        *element_map,
                    ))
                    .map_err(ExecutionError::Runtime)?;
                let visible = MirValue::Array(
                    elements
                        .iter()
                        .map(|element| value(values, *element).map(|value| value.visible.clone()))
                        .collect::<Result<_, _>>()?,
                );
                return Ok(RuntimeValue::managed(visible, reference));
            }
            MirInstructionKind::ArrayCreate {
                length,
                initial_value,
                element_map,
            } => {
                let MirValue::Integer(length) = value(values, *length)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(length) = length
                    .signed()
                    .filter(|length| *length >= 0)
                    .and_then(|length| u32::try_from(length).ok())
                else {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                    ));
                };
                let reference = self
                    .runtime
                    .allocate_array(&ArrayAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        length,
                        *element_map,
                    ))
                    .map_err(ExecutionError::Runtime)?;
                let initial_value = value(values, *initial_value)?.visible.clone();
                let mut elements = Vec::new();
                elements
                    .try_reserve_exact(length as usize)
                    .map_err(|_| ExecutionError::InvalidControlFlow)?;
                elements.resize(length as usize, initial_value);
                return Ok(RuntimeValue::managed(MirValue::Array(elements), reference));
            }
            MirInstructionKind::TableMake {
                entries,
                key_map,
                value_map,
            } => {
                let reference = self
                    .runtime
                    .allocate_table(
                        &TableAllocationRequest::new(
                            RuntimeTypeId::new(instruction.result_type().raw()),
                            AllocationClass::NurseryEligible,
                            u32::try_from(entries.len()).unwrap_or(u32::MAX),
                            *key_map,
                            *value_map,
                        )
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                    )
                    .map_err(ExecutionError::Runtime)?;
                let visible = MirValue::Table(
                    entries
                        .iter()
                        .map(|(key, entry_value)| {
                            Ok((
                                value(values, *key)?.visible.clone(),
                                value(values, *entry_value)?.visible.clone(),
                            ))
                        })
                        .collect::<Result<_, ExecutionError>>()?,
                );
                return Ok(RuntimeValue::managed(visible, reference));
            }
            MirInstructionKind::TableGet { table, key } => {
                let (MirValue::Table(entries), key) = (
                    &value(values, *table)?.visible,
                    &value(values, *key)?.visible,
                ) else {
                    return Err(ExecutionError::TypeMismatch);
                };
                return Ok(RuntimeValue::visible(
                    entries
                        .iter()
                        .find(|(candidate, _)| candidate == key)
                        .map_or(MirValue::Nil, |(_, value)| value.clone()),
                ));
            }
            MirInstructionKind::TableSet {
                table,
                key,
                value: stored,
                ..
            } => {
                let owner = value(values, *table)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let key = value(values, *key)?.visible.clone();
                let stored = value(values, *stored)?.visible.clone();
                let mut updated = false;
                for candidate in values.values_mut() {
                    if candidate.reference != Some(owner) {
                        continue;
                    }
                    let MirValue::Table(entries) = &mut candidate.visible else {
                        continue;
                    };
                    if let Some((_, current)) = entries
                        .iter_mut()
                        .find(|(candidate_key, _)| *candidate_key == key)
                    {
                        *current = stored.clone();
                    } else {
                        entries.push((key.clone(), stored.clone()));
                    }
                    updated = true;
                }
                if !updated {
                    return Err(ExecutionError::TypeMismatch);
                }
                MirValue::Nil
            }
            MirInstructionKind::ArrayGet { array, index } => {
                let (MirValue::Array(elements), MirValue::Integer(index)) = (
                    &value(values, *array)?.visible,
                    &value(values, *index)?.visible,
                ) else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if index.kind() != IntegerKind::Int64 {
                    return Err(ExecutionError::TypeMismatch);
                }
                let Some(index) = index.signed() else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(zero_based) = index
                    .checked_sub(1)
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    return Ok(RuntimeValue::visible(MirValue::Nil));
                };
                return Ok(RuntimeValue::visible(
                    elements.get(zero_based).cloned().unwrap_or(MirValue::Nil),
                ));
            }
            MirInstructionKind::ArrayLength { array } => {
                let MirValue::Array(elements) = &value(values, *array)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                MirValue::Integer(
                    IntegerValue::parse_decimal(&elements.len().to_string(), IntegerKind::Int64)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                )
            }
            MirInstructionKind::ArrayGetChecked { array, index } => {
                let (MirValue::Array(elements), MirValue::Integer(index)) = (
                    &value(values, *array)?.visible,
                    &value(values, *index)?.visible,
                ) else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(zero_based) = index
                    .signed()
                    .and_then(|value| value.checked_sub(1))
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                    ));
                };
                let Some(element) = elements.get(zero_based).cloned() else {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                    ));
                };
                element
            }
            MirInstructionKind::ArraySet {
                array,
                index,
                value: stored,
                ..
            } => {
                let owner = value(values, *array)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let MirValue::Integer(index) = value(values, *index)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(zero_based) = index
                    .signed()
                    .and_then(|value| value.checked_sub(1))
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                    ));
                };
                let stored = value(values, *stored)?.visible.clone();
                let mut updated = false;
                for candidate in values.values_mut() {
                    if candidate.reference != Some(owner) {
                        continue;
                    }
                    let MirValue::Array(elements) = &mut candidate.visible else {
                        continue;
                    };
                    let Some(slot) = elements.get_mut(zero_based) else {
                        return Err(ExecutionError::Runtime(
                            self.runtime
                                .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                        ));
                    };
                    *slot = stored.clone();
                    updated = true;
                }
                if !updated {
                    return Err(ExecutionError::TypeMismatch);
                }
                MirValue::Nil
            }
            MirInstructionKind::ArrayFill {
                array,
                value: stored,
                ..
            } => {
                let owner = value(values, *array)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let stored = value(values, *stored)?.visible.clone();
                let mut updated = false;
                for candidate in values.values_mut() {
                    if candidate.reference != Some(owner) {
                        continue;
                    }
                    let MirValue::Array(elements) = &mut candidate.visible else {
                        continue;
                    };
                    elements.fill(stored.clone());
                    updated = true;
                }
                if !updated {
                    return Err(ExecutionError::TypeMismatch);
                }
                MirValue::Nil
            }
            MirInstructionKind::ListCreate {
                capacity,
                element_map,
            } => {
                let capacity = if let Some(capacity) = capacity {
                    let MirValue::Integer(capacity) = value(values, *capacity)?.visible else {
                        return Err(ExecutionError::TypeMismatch);
                    };
                    let Some(capacity) = capacity
                        .signed()
                        .filter(|capacity| *capacity >= 0)
                        .and_then(|capacity| u32::try_from(capacity).ok())
                    else {
                        return Err(ExecutionError::Runtime(
                            self.runtime
                                .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                        ));
                    };
                    capacity
                } else {
                    0
                };
                let reference = self
                    .runtime
                    .allocate_table(
                        &TableAllocationRequest::new(
                            RuntimeTypeId::new(instruction.result_type().raw()),
                            AllocationClass::NurseryEligible,
                            capacity,
                            pop_runtime_interface::ArrayElementMap::Scalar,
                            *element_map,
                        )
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                    )
                    .map_err(ExecutionError::Runtime)?;
                return Ok(RuntimeValue::managed(MirValue::List(Vec::new()), reference));
            }
            MirInstructionKind::RangeCreate { first, last, step } => {
                let MirValue::Integer(first) = value(values, *first)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let MirValue::Integer(last) = value(values, *last)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let MirValue::Integer(step) = value(values, *step)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                if step.signed() == Some(0) || step.unsigned() == Some(0) {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::InvalidRangeStep)),
                    ));
                }
                let object_map = ObjectMap::new(3, Vec::new())
                    .map_err(|_| ExecutionError::InvalidControlFlow)?;
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        object_map,
                    ))
                    .map_err(ExecutionError::Runtime)?;
                return Ok(RuntimeValue::managed(
                    MirValue::Range { first, last, step },
                    reference,
                ));
            }
            MirInstructionKind::ListLength { list } => {
                let MirValue::List(elements) = &value(values, *list)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                MirValue::Integer(
                    IntegerValue::parse_decimal(&elements.len().to_string(), IntegerKind::Int64)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                )
            }
            MirInstructionKind::ListGet { list, index } => {
                let (MirValue::List(elements), MirValue::Integer(index)) = (
                    &value(values, *list)?.visible,
                    &value(values, *index)?.visible,
                ) else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let zero_based = index
                    .signed()
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| usize::try_from(index).ok());
                return Ok(RuntimeValue::visible(
                    zero_based
                        .and_then(|index| elements.get(index).cloned())
                        .unwrap_or(MirValue::Nil),
                ));
            }
            MirInstructionKind::ListGetChecked { list, index } => {
                let (MirValue::List(elements), MirValue::Integer(index)) = (
                    &value(values, *list)?.visible,
                    &value(values, *index)?.visible,
                ) else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(element) = index
                    .signed()
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| elements.get(index).cloned())
                else {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                    ));
                };
                element
            }
            MirInstructionKind::ListSet {
                list,
                index,
                value: stored,
                ..
            } => {
                let owner = value(values, *list)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let MirValue::Integer(index) = value(values, *index)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(zero_based) = index
                    .signed()
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| usize::try_from(index).ok())
                else {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                    ));
                };
                let stored = value(values, *stored)?.visible.clone();
                let mut updated = false;
                for candidate in values.values_mut() {
                    if candidate.reference != Some(owner) {
                        continue;
                    }
                    let MirValue::List(elements) = &mut candidate.visible else {
                        continue;
                    };
                    let Some(slot) = elements.get_mut(zero_based) else {
                        return Err(ExecutionError::Runtime(
                            self.runtime
                                .raise_trap(Trap::new(TrapKind::BoundsViolation)),
                        ));
                    };
                    *slot = stored.clone();
                    updated = true;
                }
                if !updated {
                    return Err(ExecutionError::TypeMismatch);
                }
                MirValue::Nil
            }
            MirInstructionKind::ListAdd {
                list,
                value: stored,
                ..
            } => {
                let owner = value(values, *list)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let stored = value(values, *stored)?.visible.clone();
                let mut updated = false;
                for candidate in values.values_mut() {
                    if candidate.reference != Some(owner) {
                        continue;
                    }
                    let MirValue::List(elements) = &mut candidate.visible else {
                        continue;
                    };
                    elements.push(stored.clone());
                    updated = true;
                }
                if !updated {
                    return Err(ExecutionError::TypeMismatch);
                }
                MirValue::Nil
            }
            MirInstructionKind::BooleanNot { operand } => match &value(values, *operand)?.visible {
                MirValue::Boolean(value) => MirValue::Boolean(!value),
                _ => return Err(ExecutionError::TypeMismatch),
            },
            MirInstructionKind::BooleanAnd { left, right } => {
                return boolean_binary(values, *left, *right, |left, right| left && right)
                    .map(RuntimeValue::visible);
            }
            MirInstructionKind::BooleanOr { left, right } => {
                return boolean_binary(values, *left, *right, |left, right| left || right)
                    .map(RuntimeValue::visible);
            }
            MirInstructionKind::CompareEqual { left, right } => MirValue::Boolean(pop_value_equal(
                &value(values, *left)?.visible,
                &value(values, *right)?.visible,
            )),
            MirInstructionKind::CompareNotEqual { left, right } => {
                MirValue::Boolean(!pop_value_equal(
                    &value(values, *left)?.visible,
                    &value(values, *right)?.visible,
                ))
            }
            MirInstructionKind::FfiHandleOpen { value: managed } => {
                let managed = value(values, *managed)?.clone();
                let reference = managed.reference.ok_or(ExecutionError::TypeMismatch)?;
                let handle = self
                    .runtime
                    .retain_root(reference)
                    .map_err(ExecutionError::Runtime)?;
                if handle.raw() == 0 {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::ImpossibleState)),
                    ));
                }
                self.ffi_handles.insert(handle, managed);
                MirValue::FfiHandle(handle.raw())
            }
            MirInstructionKind::FfiHandleGet { handle } => {
                let MirValue::FfiHandle(raw) = value(values, *handle)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let handle = RootHandle::new(raw);
                let reference = self
                    .runtime
                    .resolve_root(handle)
                    .map_err(ExecutionError::Runtime)?;
                if reference.raw() == 0 {
                    return Err(ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::ImpossibleState)),
                    ));
                }
                let managed = self.ffi_handles.get_mut(&handle).ok_or_else(|| {
                    ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::ImpossibleState)),
                    )
                })?;
                managed.install_relocated_reference(Some(reference))?;
                return Ok(managed.clone());
            }
            MirInstructionKind::FfiCallbackOpenScoped {
                callback,
                owner,
                function,
                site,
                ..
            } => {
                let callback = value(values, *callback)?;
                let reference = callback.reference.ok_or(ExecutionError::TypeMismatch)?;
                let target = self.interpreter_callback_target(callback, *owner, *function)?;
                let site = runtime_callback_site(*owner, *site)?;
                let request = FfiCallbackOpenRequest::new(
                    Some(reference),
                    site,
                    SchedulerId::new(1),
                    FfiCallbackLifetime::CallScoped,
                    FfiCallbackThread::CallingThread,
                );
                let registration = match self.runtime.ffi_callback_open(request) {
                    Ok(registration) => registration,
                    Err(
                        FfiCallbackOpenFailure::Allocation | FfiCallbackOpenFailure::Invariant(_),
                    ) => {
                        return Err(self.runtime_invariant());
                    }
                };
                self.ffi_callbacks.insert(
                    registration.id(),
                    InterpreterCallback {
                        registration,
                        site,
                        target,
                        environment: reference,
                        closed: false,
                    },
                );
                return Ok(RuntimeValue::managed(
                    MirValue::FfiRegisteredCallback {
                        registration: registration.id().raw(),
                        reference,
                    },
                    reference,
                ));
            }
            MirInstructionKind::FfiCallbackOpenOwned {
                callback,
                owner,
                function,
                site,
                thread,
                result,
                success,
                failure,
                ..
            } => {
                let callback = value(values, *callback)?;
                let reference = callback.reference.ok_or(ExecutionError::TypeMismatch)?;
                let target = self.interpreter_callback_target(callback, *owner, *function)?;
                let site = runtime_callback_site(*owner, *site)?;
                let request = FfiCallbackOpenRequest::new(
                    Some(reference),
                    site,
                    SchedulerId::new(1),
                    FfiCallbackLifetime::Registered,
                    *thread,
                );
                let visible = match self.runtime.ffi_callback_open(request) {
                    Ok(registration) => {
                        self.ffi_callbacks.insert(
                            registration.id(),
                            InterpreterCallback {
                                registration,
                                site,
                                target,
                                environment: reference,
                                closed: false,
                            },
                        );
                        MirValue::Result {
                            definition: *result,
                            case: *success,
                            arguments: vec![MirValue::FfiRegisteredCallback {
                                registration: registration.id().raw(),
                                reference,
                            }],
                        }
                    }
                    Err(FfiCallbackOpenFailure::Allocation) => MirValue::Result {
                        definition: *result,
                        case: *failure,
                        arguments: vec![MirValue::FfiCallbackOpenError],
                    },
                    Err(FfiCallbackOpenFailure::Invariant(_)) => {
                        return Err(self.runtime_invariant());
                    }
                };
                return Ok(RuntimeValue::visible(visible));
            }
            MirInstructionKind::FfiCallbackCloseOwned {
                callback,
                result,
                success,
                failure,
            } => {
                let MirValue::FfiRegisteredCallback { registration, .. } =
                    value(values, *callback)?.visible
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let id = FfiCallbackRegistrationId::new(registration)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let state = self
                    .ffi_callbacks
                    .get_mut(&id)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let case = if state.closed {
                    *success
                } else {
                    match self.runtime.ffi_callback_close(
                        id,
                        state.registration.context(),
                        state.site,
                    ) {
                        Ok(()) => {
                            state.closed = true;
                            *success
                        }
                        Err(FfiCallbackCloseFailure::InUse) => *failure,
                        Err(FfiCallbackCloseFailure::Invariant(_)) => {
                            return Err(self.runtime_invariant());
                        }
                    }
                };
                let arguments = if case == *success {
                    vec![MirValue::Nil]
                } else {
                    vec![MirValue::FfiCallbackInUseError]
                };
                MirValue::Result {
                    definition: *result,
                    case,
                    arguments,
                }
            }
            MirInstructionKind::FfiBufferOpen {
                length,
                element_size,
                alignment,
                layout,
                result,
                success,
                failure,
                ..
            } => {
                let length = integer_u64(&value(values, *length)?.visible)?;
                let request = FfiBufferOpenRequest::new(length, *element_size, *alignment, *layout)
                    .map_err(|_| self.runtime_invariant())?;
                match self.runtime.ffi_buffer_open(&request) {
                    Ok(reference) if reference.raw() != 0 => MirValue::Result {
                        definition: *result,
                        case: *success,
                        arguments: vec![MirValue::FfiBuffer(reference)],
                    },
                    Ok(_) | Err(FfiBufferOpenFailure::Invariant(_)) => {
                        return Err(self.runtime_invariant());
                    }
                    Err(FfiBufferOpenFailure::Allocation) => MirValue::Result {
                        definition: *result,
                        case: *failure,
                        arguments: vec![MirValue::FfiAllocationError],
                    },
                }
            }
            MirInstructionKind::FfiBufferLength { buffer, layout } => {
                let reference = value(values, *buffer)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let length = self
                    .runtime
                    .ffi_buffer_length(reference, *layout)
                    .map_err(|_| self.runtime_invariant())?;
                MirValue::Integer(
                    IntegerValue::parse_decimal(&length.to_string(), IntegerKind::UInt64)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                )
            }
            MirInstructionKind::FfiBufferRead {
                buffer,
                index,
                layout,
            } => {
                let reference = value(values, *buffer)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let index = integer_u64(&value(values, *index)?.visible)?;
                let entry = self
                    .mir
                    .ffi_layouts()
                    .get(*layout)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let mut bytes = vec![
                    0;
                    usize::try_from(entry.size())
                        .map_err(|_| ExecutionError::InvalidControlFlow)?
                ];
                self.runtime
                    .ffi_buffer_read(reference, *layout, index, &mut bytes)
                    .map_err(|_| self.runtime_invariant())?;
                unmarshal(&bytes, entry, self.mir.ffi_layouts(), self.arena, self.mir)?
            }
            MirInstructionKind::FfiBufferBorrow {
                buffer,
                expected_length,
                layout,
                region,
            } => {
                let reference = value(values, *buffer)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let expected = integer_u64(&value(values, *expected_length)?.visible)?;
                let borrow = self
                    .runtime
                    .ffi_buffer_borrow(reference, *layout)
                    .map_err(|_| self.runtime_invariant())?;
                if borrow.length() != expected
                    || self
                        .ffi_buffer_borrows
                        .insert(*region, borrow.id())
                        .is_some()
                {
                    return Err(self.runtime_invariant());
                }
                borrow.address().map_or(MirValue::Nil, MirValue::FfiPointer)
            }
            MirInstructionKind::FfiBytesBorrow { bytes, region } => {
                if self.ffi_bytes_borrows.contains_key(region) {
                    return Err(self.runtime_invariant());
                }
                let owner = value(values, *bytes)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let borrow = self
                    .runtime
                    .ffi_bytes_borrow(owner)
                    .map_err(|_| self.runtime_invariant())?;
                let state = FfiBytesBorrowState {
                    owner,
                    borrow: borrow.id(),
                    length: borrow.length(),
                };
                self.ffi_bytes_borrows.insert(*region, state);
                borrow.address().map_or(MirValue::Nil, MirValue::FfiPointer)
            }
            MirInstructionKind::FfiBytesBorrowLength { bytes, region } => {
                let owner = value(values, *bytes)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let state = self
                    .ffi_bytes_borrows
                    .get(region)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                if state.owner != owner {
                    return Err(self.runtime_invariant());
                }
                MirValue::Integer(
                    IntegerValue::parse_decimal(&state.length.to_string(), IntegerKind::UInt64)
                        .map_err(|_| ExecutionError::InvalidControlFlow)?,
                )
            }
            MirInstructionKind::FfiUnsafeLoad { pointer, layout } => {
                let address = ffi_pointer(&value(values, *pointer)?.visible)?;
                let entry = self
                    .mir
                    .ffi_layouts()
                    .get(*layout)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.verify_ffi_alignment(address, entry.alignment())?;
                let mut bytes = vec![
                    0;
                    usize::try_from(entry.size())
                        .map_err(|_| ExecutionError::InvalidControlFlow)?
                ];
                self.runtime
                    .ffi_unsafe_read(address, &mut bytes)
                    .map_err(|_| self.runtime_invariant())?;
                unmarshal(&bytes, entry, self.mir.ffi_layouts(), self.arena, self.mir)?
            }
            MirInstructionKind::FfiUnsafeAdvance {
                pointer,
                elements,
                layout,
                ..
            } => {
                let address = ffi_pointer(&value(values, *pointer)?.visible)?;
                let elements = integer_i64(&value(values, *elements)?.visible)?;
                let entry = self
                    .mir
                    .ffi_layouts()
                    .get(*layout)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let offset = i128::from(elements)
                    .checked_mul(i128::from(entry.size()))
                    .ok_or_else(|| self.integer_overflow())?;
                let raw = i128::from(address.raw())
                    .checked_add(offset)
                    .and_then(|raw| u64::try_from(raw).ok())
                    .and_then(ForeignAddress::new)
                    .ok_or_else(|| self.integer_overflow())?;
                self.runtime
                    .ffi_unsafe_read(raw, &mut [])
                    .map_err(|_| self.runtime_invariant())?;
                MirValue::FfiPointer(raw)
            }
            MirInstructionKind::FfiUnsafeAddress { pointer, .. } => {
                let address = ffi_pointer(&value(values, *pointer)?.visible)?;
                MirValue::Integer(integer_from_u64(
                    address.raw(),
                    instruction.result_type(),
                    self.mir.ffi_layouts(),
                    self.arena,
                )?)
            }
            MirInstructionKind::FfiUnsafePointerFromAddress { address, .. } => {
                let raw = integer_u64(&value(values, *address)?.visible)?;
                ForeignAddress::new(raw).map_or(MirValue::Nil, MirValue::FfiPointer)
            }
            MirInstructionKind::IntegerConstant(_)
            | MirInstructionKind::FloatConstant(_)
            | MirInstructionKind::CheckedIntegerAdd { .. }
            | MirInstructionKind::CheckedIntegerSubtract { .. }
            | MirInstructionKind::CheckedIntegerMultiply { .. }
            | MirInstructionKind::CheckedIntegerDivide { .. }
            | MirInstructionKind::CheckedIntegerRemainder { .. }
            | MirInstructionKind::FloatAdd { .. }
            | MirInstructionKind::FloatSubtract { .. }
            | MirInstructionKind::FloatMultiply { .. }
            | MirInstructionKind::FloatDivide { .. }
            | MirInstructionKind::IntegerNegate { .. }
            | MirInstructionKind::FloatNegate { .. }
            | MirInstructionKind::ConvertInteger { .. }
            | MirInstructionKind::ConvertIntegerToFloat { .. }
            | MirInstructionKind::ConvertFloatToInteger { .. }
            | MirInstructionKind::ConvertFloat { .. }
            | MirInstructionKind::CompareIntegerLess { .. }
            | MirInstructionKind::CompareIntegerLessOrEqual { .. }
            | MirInstructionKind::CompareIntegerGreater { .. }
            | MirInstructionKind::CompareIntegerGreaterOrEqual { .. }
            | MirInstructionKind::CompareFloatLess { .. }
            | MirInstructionKind::CompareFloatLessOrEqual { .. }
            | MirInstructionKind::CompareFloatGreater { .. }
            | MirInstructionKind::CompareFloatGreaterOrEqual { .. }
            | MirInstructionKind::CallStandard { .. }
            | MirInstructionKind::CallDirect { .. }
            | MirInstructionKind::CallForeign { .. }
            | MirInstructionKind::CallReferenced { .. }
            | MirInstructionKind::CallDirectMethod { .. }
            | MirInstructionKind::CallInterface { .. }
            | MirInstructionKind::CallBuiltinInterface { .. }
            | MirInstructionKind::CallIndirect { .. }
            | MirInstructionKind::CallScopedBorrow { .. }
            | MirInstructionKind::CallCallbackPair { .. }
            | MirInstructionKind::RecordMake { .. }
            | MirInstructionKind::ClassMake { .. }
            | MirInstructionKind::RecordUpdate { .. }
            | MirInstructionKind::FieldGet { .. }
            | MirInstructionKind::FieldSet { .. }
            | MirInstructionKind::UnionMake { .. }
            | MirInstructionKind::ResultMake { .. }
            | MirInstructionKind::IterationMake { .. }
            | MirInstructionKind::ErrorMake { .. }
            | MirInstructionKind::InterfaceUpcast { .. }
            | MirInstructionKind::CheckedDowncast { .. }
            | MirInstructionKind::ViewEnd { .. }
            | MirInstructionKind::CaptureCellAllocate { .. }
            | MirInstructionKind::CaptureCellLoad { .. }
            | MirInstructionKind::CaptureCellStore { .. }
            | MirInstructionKind::ClosureEnvironmentAllocate { .. }
            | MirInstructionKind::CaptureLoad { .. }
            | MirInstructionKind::CaptureCellReference { .. }
            | MirInstructionKind::CaptureStore { .. }
            | MirInstructionKind::GcSafePoint { .. }
            | MirInstructionKind::RetainRoot { .. }
            | MirInstructionKind::ReleaseRoot { .. }
            | MirInstructionKind::FfiHandleClose { .. }
            | MirInstructionKind::FfiCallbackCloseScoped { .. }
            | MirInstructionKind::FfiBufferWrite { .. }
            | MirInstructionKind::FfiBufferEndBorrow { .. }
            | MirInstructionKind::FfiBytesEndBorrow { .. }
            | MirInstructionKind::FfiBufferClose { .. }
            | MirInstructionKind::FfiUnsafeStore { .. }
            | MirInstructionKind::FfiUnsafeCopy { .. }
            | MirInstructionKind::Pin { .. }
            | MirInstructionKind::Unpin { .. }
            | MirInstructionKind::WriteBarrier { .. } => {
                return Err(ExecutionError::InvalidControlFlow);
            }
        };
        Ok(RuntimeValue::visible(result))
    }

    fn runtime_invariant(&mut self) -> ExecutionError {
        ExecutionError::Runtime(
            self.runtime
                .raise_trap(Trap::new(TrapKind::ImpossibleState)),
        )
    }

    fn bounds_violation(&mut self) -> ExecutionError {
        ExecutionError::Runtime(
            self.runtime
                .raise_trap(Trap::new(TrapKind::BoundsViolation)),
        )
    }

    fn integer_overflow(&mut self) -> ExecutionError {
        ExecutionError::Runtime(
            self.runtime
                .raise_trap(Trap::new(TrapKind::IntegerOverflow)),
        )
    }

    fn verify_ffi_alignment(
        &mut self,
        address: ForeignAddress,
        alignment: u64,
    ) -> Result<(), ExecutionError> {
        if alignment != 0 && address.raw().is_multiple_of(alignment) {
            Ok(())
        } else {
            Err(self.runtime_invariant())
        }
    }

    fn evaluate_effect_instruction(
        &mut self,
        instruction: &pop_mir::MirInstruction,
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<(), ExecutionError> {
        let returned = match instruction.kind() {
            MirInstructionKind::ViewEnd { .. } => return Ok(()),
            MirInstructionKind::CallStandard {
                function,
                arguments,
                ..
            } => {
                if arguments.len() != 1 {
                    return Err(ExecutionError::InvalidControlFlow);
                }
                match (function.raw(), &value(values, arguments[0])?.visible) {
                    (0, MirValue::Integer(value)) => {
                        let value = value.signed().ok_or(ExecutionError::TypeMismatch)?;
                        pop_standard::pop_std_print_int(value);
                    }
                    (1, MirValue::String(value)) => pop_standard::print_string(value),
                    (0 | 1, _) => return Err(ExecutionError::TypeMismatch),
                    _ => return Err(ExecutionError::InvalidControlFlow),
                }
                return Ok(());
            }
            MirInstructionKind::CallDirect {
                function,
                arguments,
                ..
            } => self.execute_direct_call(*function, arguments, values)?,
            MirInstructionKind::CallForeign {
                function,
                arguments,
                roots,
                safe_point,
                ..
            } => self.execute_foreign_call(
                *function,
                arguments,
                roots,
                *safe_point,
                instruction.effects(),
                values,
            )?,
            MirInstructionKind::CallReferenced { function, .. } => {
                return Err(ExecutionError::UnknownReferencedFunction(*function));
            }
            MirInstructionKind::CallDirectMethod {
                method, arguments, ..
            } => self.execute_method_call(*method, arguments, values)?,
            MirInstructionKind::CallIndirect {
                callee, arguments, ..
            } => self.execute_indirect_call(*callee, arguments, values)?,
            MirInstructionKind::CallInterface {
                method, arguments, ..
            } => {
                let receiver = arguments.first().ok_or(ExecutionError::WrongArity)?;
                let MirValue::Class(class) = &value(values, *receiver)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let implementation = self
                    .mir
                    .declarations()
                    .iter()
                    .find_map(|declaration| match declaration.kind() {
                        pop_mir::MirDeclarationKind::Class(class_declaration)
                            if class_declaration.class() == class.class() =>
                        {
                            class_declaration
                                .interfaces()
                                .iter()
                                .flat_map(pop_mir::MirInterfaceImplementation::methods)
                                .find(|candidate| candidate.interface_method() == *method)
                                .map(|candidate| candidate.class_method())
                        }
                        _ => None,
                    })
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.execute_method_call(implementation, arguments, values)?
            }
            MirInstructionKind::CaptureCellStore {
                cell,
                value: stored,
            } => {
                let MirValue::Function(symbol) = value(values, *cell)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(PrivateValue::Cell(cell)) = self.private_values.get(&symbol) else {
                    return Err(ExecutionError::TypeMismatch);
                };
                *cell.borrow_mut() = value(values, *stored)?.clone();
                return Ok(());
            }
            MirInstructionKind::CaptureStore {
                capture,
                value: stored,
                ..
            } => {
                let environment = self
                    .active_captures
                    .as_ref()
                    .ok_or(ExecutionError::InvalidControlFlow)?
                    .clone();
                let slot = capture.raw() as usize;
                let stored = value(values, *stored)?.clone();
                let mut captures = environment.borrow_mut();
                let target = captures
                    .get_mut(slot)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                if let MirValue::Function(symbol) = &target.visible
                    && let Some(PrivateValue::Cell(cell)) = self.private_values.get(symbol)
                {
                    *cell.borrow_mut() = stored;
                } else {
                    *target = stored;
                }
                return Ok(());
            }
            MirInstructionKind::GcSafePoint {
                roots, stack_map, ..
            } => {
                let published_values = roots
                    .iter()
                    .map(|root| value(values, *root).map(|value| value.reference))
                    .collect::<Result<_, _>>()?;
                let mut publication = RootPublication::new(stack_map.clone(), published_values)
                    .map_err(|_| ExecutionError::InvalidControlFlow)?;
                self.runtime
                    .safe_point(&mut publication)
                    .map_err(ExecutionError::Runtime)?;
                install_published_relocations(roots, &publication, values)?;
                return Ok(());
            }
            MirInstructionKind::RetainRoot { value: root } => {
                let reference = value(values, *root)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let handle = self
                    .runtime
                    .retain_root(reference)
                    .map_err(ExecutionError::Runtime)?;
                self.root_handles.insert(instruction.result(), handle);
                return Ok(());
            }
            MirInstructionKind::ReleaseRoot { handle } => {
                let handle = self
                    .root_handles
                    .remove(handle)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.runtime
                    .release_root(handle)
                    .map_err(ExecutionError::Runtime)?;
                return Ok(());
            }
            MirInstructionKind::FfiHandleClose { handle } => {
                let MirValue::FfiHandle(raw) = value(values, *handle)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let handle = RootHandle::new(raw);
                self.runtime
                    .release_root(handle)
                    .map_err(ExecutionError::Runtime)?;
                self.ffi_handles.remove(&handle).ok_or_else(|| {
                    ExecutionError::Runtime(
                        self.runtime
                            .raise_trap(Trap::new(TrapKind::ImpossibleState)),
                    )
                })?;
                return Ok(());
            }
            MirInstructionKind::FfiBufferWrite {
                buffer,
                index,
                value: stored,
                layout,
            } => {
                let reference = value(values, *buffer)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let index = integer_u64(&value(values, *index)?.visible)?;
                let entry = self
                    .mir
                    .ffi_layouts()
                    .get(*layout)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let bytes = marshal(
                    &value(values, *stored)?.visible,
                    entry,
                    self.mir.ffi_layouts(),
                )?;
                self.runtime
                    .ffi_buffer_write(reference, *layout, index, &bytes)
                    .map_err(|_| self.runtime_invariant())?;
                return Ok(());
            }
            MirInstructionKind::FfiBufferEndBorrow { buffer, region } => {
                let reference = value(values, *buffer)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let borrow = self
                    .ffi_buffer_borrows
                    .get(region)
                    .copied()
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.runtime
                    .ffi_buffer_end_borrow(reference, borrow)
                    .map_err(|_| self.runtime_invariant())?;
                self.ffi_buffer_borrows.remove(region);
                return Ok(());
            }
            MirInstructionKind::FfiBytesEndBorrow { bytes, region } => {
                let owner = value(values, *bytes)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let state = self
                    .ffi_bytes_borrows
                    .get(region)
                    .copied()
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                if state.owner != owner {
                    return Err(self.runtime_invariant());
                }
                self.runtime
                    .ffi_bytes_end_borrow(owner, state.borrow)
                    .map_err(|_| self.runtime_invariant())?;
                self.ffi_bytes_borrows.remove(region);
                return Ok(());
            }
            MirInstructionKind::FfiCallbackCloseScoped { callback, .. } => {
                let MirValue::FfiRegisteredCallback { registration, .. } =
                    &value(values, *callback)?.visible
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let id = FfiCallbackRegistrationId::new(*registration)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let state = self
                    .ffi_callbacks
                    .get_mut(&id)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                if state.closed {
                    return Err(ExecutionError::InvalidControlFlow);
                }
                match self
                    .runtime
                    .ffi_callback_close(id, state.registration.context(), state.site)
                {
                    Ok(()) => state.closed = true,
                    Err(FfiCallbackCloseFailure::InUse | FfiCallbackCloseFailure::Invariant(_)) => {
                        return Err(self.runtime_invariant());
                    }
                }
                return Ok(());
            }
            MirInstructionKind::FfiBufferClose { buffer } => {
                let reference = value(values, *buffer)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                self.runtime
                    .ffi_buffer_close(reference)
                    .map_err(|_| self.runtime_invariant())?;
                return Ok(());
            }
            MirInstructionKind::FfiUnsafeStore {
                pointer,
                value: stored,
                layout,
            } => {
                let address = ffi_pointer(&value(values, *pointer)?.visible)?;
                let entry = self
                    .mir
                    .ffi_layouts()
                    .get(*layout)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.verify_ffi_alignment(address, entry.alignment())?;
                let bytes = marshal(
                    &value(values, *stored)?.visible,
                    entry,
                    self.mir.ffi_layouts(),
                )?;
                self.runtime
                    .ffi_unsafe_write(address, &bytes)
                    .map_err(|_| self.runtime_invariant())?;
                return Ok(());
            }
            MirInstructionKind::FfiUnsafeCopy {
                source,
                destination,
                count,
                layout,
            } => {
                let source = ffi_pointer(&value(values, *source)?.visible)?;
                let destination = ffi_pointer(&value(values, *destination)?.visible)?;
                let count = integer_u64(&value(values, *count)?.visible)?;
                let entry = self
                    .mir
                    .ffi_layouts()
                    .get(*layout)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.verify_ffi_alignment(source, entry.alignment())?;
                self.verify_ffi_alignment(destination, entry.alignment())?;
                let byte_count = count
                    .checked_mul(entry.size())
                    .ok_or_else(|| self.integer_overflow())?;
                self.runtime
                    .ffi_unsafe_copy(source, destination, byte_count)
                    .map_err(|_| self.runtime_invariant())?;
                return Ok(());
            }
            MirInstructionKind::Pin { value: pinned } => {
                let reference = value(values, *pinned)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let handle = self
                    .runtime
                    .pin(reference)
                    .map_err(ExecutionError::Runtime)?;
                self.pin_handles.insert(instruction.result(), handle);
                return Ok(());
            }
            MirInstructionKind::Unpin { handle } => {
                let handle = self
                    .pin_handles
                    .remove(handle)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.runtime
                    .unpin(handle)
                    .map_err(ExecutionError::Runtime)?;
                return Ok(());
            }
            MirInstructionKind::WriteBarrier {
                owner,
                slot,
                previous,
                value: stored,
                proof,
            } => {
                if proof.is_some() {
                    return Ok(());
                }
                let owner = value(values, *owner)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let previous = previous
                    .map(|previous| value(values, previous).map(|value| value.reference))
                    .transpose()?
                    .flatten();
                let stored = stored
                    .map(|stored| value(values, stored).map(|value| value.reference))
                    .transpose()?
                    .flatten();
                self.runtime
                    .write_barrier(WriteBarrier::new(
                        BarrierKind::CombinedSatbGenerational,
                        owner,
                        *slot,
                        previous,
                        stored,
                    ))
                    .map_err(ExecutionError::Runtime)?;
                return Ok(());
            }
            _ => return Err(ExecutionError::InvalidControlFlow),
        };
        if returned.is_empty() {
            Ok(())
        } else {
            Err(ExecutionError::WrongArity)
        }
    }

    fn evaluate_structured_instruction(
        &mut self,
        instruction: &MirInstruction,
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Option<RuntimeValue>, ExecutionError> {
        let result = match instruction.kind() {
            MirInstructionKind::CallDirect {
                function,
                arguments,
                ..
            } => single_result(self.execute_direct_call(*function, arguments, values)?),
            MirInstructionKind::CallForeign {
                function,
                arguments,
                roots,
                safe_point,
                ..
            } => single_result(self.execute_foreign_call(
                *function,
                arguments,
                roots,
                *safe_point,
                instruction.effects(),
                values,
            )?),
            MirInstructionKind::CallReferenced { function, .. } => {
                return Err(ExecutionError::UnknownReferencedFunction(*function));
            }
            MirInstructionKind::CallDirectMethod {
                method, arguments, ..
            } => single_result(self.execute_method_call(*method, arguments, values)?),
            MirInstructionKind::CallIndirect {
                callee, arguments, ..
            } => single_result(self.execute_indirect_call(*callee, arguments, values)?),
            MirInstructionKind::CallScopedBorrow {
                owner,
                function,
                captures,
                arguments,
                ..
            } => single_result(
                self.execute_scoped_borrow_call(*owner, *function, captures, arguments, values)?,
            ),
            MirInstructionKind::CallCallbackPair {
                callback,
                owner,
                function,
                captures,
                lifetime,
                result,
                success,
                failure,
                ..
            } => {
                let MirValue::FfiRegisteredCallback { registration, .. } =
                    &value(values, *callback)?.visible
                else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let id = FfiCallbackRegistrationId::new(*registration)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let state = self
                    .ffi_callbacks
                    .get(&id)
                    .cloned()
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                if state.closed {
                    if *lifetime != FfiCallbackLifetime::Registered {
                        return Err(ExecutionError::InvalidControlFlow);
                    }
                    let (Some(result), Some(failure)) = (result, failure) else {
                        return Err(ExecutionError::InvalidControlFlow);
                    };
                    return Ok(Some(RuntimeValue::visible(MirValue::Result {
                        definition: *result,
                        case: *failure,
                        arguments: vec![MirValue::FfiCallbackClosedError],
                    })));
                }
                let arguments = [
                    RuntimeValue::visible(MirValue::FfiFunction(state.site.raw())),
                    RuntimeValue::visible(MirValue::FfiPointer(state.registration.context())),
                ];
                let returned = self
                    .execute_callback_pair_call(*owner, *function, captures, &arguments, values)?;
                let returned = single_result(returned)?;
                if *lifetime == FfiCallbackLifetime::CallScoped {
                    Ok(returned)
                } else {
                    let (Some(result), Some(success)) = (result, success) else {
                        return Err(ExecutionError::InvalidControlFlow);
                    };
                    Ok(RuntimeValue::visible(MirValue::Result {
                        definition: *result,
                        case: *success,
                        arguments: vec![returned.visible],
                    }))
                }
            }
            MirInstructionKind::CallInterface {
                method, arguments, ..
            } => {
                let receiver = arguments.first().ok_or(ExecutionError::WrongArity)?;
                let MirValue::Class(class) = &value(values, *receiver)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let implementation = self
                    .mir
                    .declarations()
                    .iter()
                    .find_map(|declaration| match declaration.kind() {
                        pop_mir::MirDeclarationKind::Class(class_declaration)
                            if class_declaration.class() == class.class() =>
                        {
                            class_declaration
                                .interfaces()
                                .iter()
                                .flat_map(pop_mir::MirInterfaceImplementation::methods)
                                .find(|implementation| implementation.interface_method() == *method)
                                .map(|implementation| implementation.class_method())
                        }
                        _ => None,
                    })
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                single_result(self.execute_method_call(implementation, arguments, values)?)
            }
            MirInstructionKind::CallBuiltinInterface {
                method, arguments, ..
            } => {
                if arguments.len() != 1 {
                    return Err(ExecutionError::WrongArity);
                }
                let receiver = value(values, arguments[0])?.clone();
                if let MirValue::Class(class) = &receiver.visible {
                    let implementation = self
                        .mir
                        .declarations()
                        .iter()
                        .find_map(|declaration| match declaration.kind() {
                            pop_mir::MirDeclarationKind::Class(class_declaration)
                                if class_declaration.class() == class.class() =>
                            {
                                class_declaration
                                    .builtin_interfaces()
                                    .iter()
                                    .flat_map(pop_mir::MirBuiltinInterfaceImplementation::methods)
                                    .find(|implementation| {
                                        implementation.protocol_method() == *method
                                    })
                                    .map(|implementation| implementation.class_method())
                            }
                            _ => None,
                        })
                        .ok_or(ExecutionError::InvalidControlFlow)?;
                    return single_result(self.execute_method_call(
                        implementation,
                        arguments,
                        values,
                    )?)
                    .map(Some);
                }
                if method.raw() == 0 {
                    if let MirValue::Function(symbol) = &receiver.visible
                        && matches!(
                            self.private_values.get(symbol),
                            Some(PrivateValue::Iterator { .. })
                        )
                    {
                        Ok(receiver)
                    } else {
                        self.allocate_iteration_session(instruction.result_type(), receiver)
                    }
                } else if method.raw() == 1 {
                    self.advance_iteration_session(instruction.result_type(), &receiver, values)
                } else {
                    return Err(ExecutionError::InvalidControlFlow);
                }
            }
            MirInstructionKind::CaptureCellAllocate {
                initial,
                object_map,
                ..
            } => {
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        object_map.clone(),
                    ))
                    .map_err(ExecutionError::Runtime)?;
                let cell = Rc::new(RefCell::new(value(values, *initial)?.clone()));
                let symbol = self.fresh_private_symbol();
                self.private_values.insert(symbol, PrivateValue::Cell(cell));
                Ok(RuntimeValue::managed(MirValue::Function(symbol), reference))
            }
            MirInstructionKind::CaptureCellLoad { cell } => {
                let MirValue::Function(symbol) = value(values, *cell)?.visible else {
                    return Err(ExecutionError::TypeMismatch);
                };
                let Some(PrivateValue::Cell(cell)) = self.private_values.get(&symbol) else {
                    return Err(ExecutionError::TypeMismatch);
                };
                Ok(cell.borrow().clone())
            }
            MirInstructionKind::CaptureLoad { capture, .. } => {
                let environment = self
                    .active_captures
                    .as_ref()
                    .ok_or(ExecutionError::InvalidControlFlow)?
                    .borrow();
                let captured = environment
                    .get(capture.raw() as usize)
                    .ok_or(ExecutionError::InvalidControlFlow)?
                    .clone();
                let MirValue::Function(symbol) = captured.visible else {
                    return Ok(Some(captured));
                };
                match self.private_values.get(&symbol) {
                    Some(PrivateValue::Cell(cell)) => Ok(cell.borrow().clone()),
                    Some(PrivateValue::Closure { .. } | PrivateValue::Iterator { .. }) => {
                        Ok(captured)
                    }
                    Some(
                        PrivateValue::Task(_)
                        | PrivateValue::CancellationSource(_)
                        | PrivateValue::CancellationToken(_)
                        | PrivateValue::TaskGroup(_),
                    ) => Err(ExecutionError::TypeMismatch),
                    None => Err(ExecutionError::TypeMismatch),
                }
            }
            MirInstructionKind::CaptureCellReference { capture, .. } => {
                let captures = self
                    .active_captures
                    .as_ref()
                    .ok_or(ExecutionError::InvalidControlFlow)?
                    .borrow();
                captures
                    .get(capture.raw() as usize)
                    .cloned()
                    .ok_or(ExecutionError::InvalidControlFlow)
            }
            MirInstructionKind::ClosureEnvironmentAllocate {
                owner,
                function,
                captures,
                object_map,
                ..
            } => {
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        object_map.clone(),
                    ))
                    .map_err(ExecutionError::Runtime)?;
                let self_slots: Vec<_> = captures
                    .iter()
                    .filter(|capture| capture.self_reference())
                    .map(|capture| capture.slot() as usize)
                    .collect();
                let environment_values = captures
                    .iter()
                    .map(|capture| {
                        if capture.self_reference() {
                            Ok(RuntimeValue::visible(MirValue::Nil))
                        } else {
                            value(values, capture.value()).cloned()
                        }
                    })
                    .collect::<Result<Vec<_>, ExecutionError>>()?;
                let symbol = self.fresh_private_symbol();
                let environment = Rc::new(RefCell::new(environment_values));
                self.private_values.insert(
                    symbol,
                    PrivateValue::Closure {
                        owner: *owner,
                        function: *function,
                        captures: environment.clone(),
                    },
                );
                let closure = RuntimeValue::managed(MirValue::Function(symbol), reference);
                for slot in self_slots {
                    environment.borrow_mut()[slot] = closure.clone();
                }
                Ok(closure)
            }
            MirInstructionKind::RecordMake { record, fields } => {
                Ok(RuntimeValue::visible(MirValue::Record {
                    record: *record,
                    fields: evaluate_visible_fields(fields, values)?,
                }))
            }
            MirInstructionKind::ClassMake {
                class,
                fields,
                object_map,
            } => {
                let definition = canonical_class_identity(
                    self.mir,
                    self.arena,
                    *class,
                    instruction.result_type(),
                )
                .ok_or(ExecutionError::InvalidControlFlow)?;
                let reference = self
                    .runtime
                    .allocate_object(&ObjectAllocationRequest::new(
                        RuntimeTypeId::new(instruction.result_type().raw()),
                        AllocationClass::NurseryEligible,
                        object_map.clone(),
                    ))
                    .map_err(ExecutionError::Runtime)?;
                Ok(RuntimeValue::managed(
                    MirValue::Class(MirClassValue::new(
                        *class,
                        definition,
                        reference,
                        evaluate_fields(fields, values)?,
                    )),
                    reference,
                ))
            }
            MirInstructionKind::RecordUpdate {
                record,
                base,
                fields,
            } => update_record(*record, *base, fields, values),
            MirInstructionKind::FieldGet { base, field } => get_field(*base, *field, values),
            MirInstructionKind::FieldSet {
                base,
                field,
                value: new_value,
            } => set_field(*base, *field, *new_value, values),
            MirInstructionKind::UnionMake {
                union,
                case,
                arguments,
            } => Ok(RuntimeValue::visible(MirValue::Union {
                union: *union,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| value(values, *argument).map(|value| value.visible.clone()))
                    .collect::<Result<_, _>>()?,
            })),
            MirInstructionKind::ResultMake {
                result,
                case,
                arguments,
            } => Ok(RuntimeValue::visible(MirValue::Result {
                definition: *result,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| value(values, *argument).map(|value| value.visible.clone()))
                    .collect::<Result<_, _>>()?,
            })),
            MirInstructionKind::IterationMake {
                iteration,
                case,
                arguments,
            } => Ok(RuntimeValue::visible(MirValue::Iteration {
                definition: *iteration,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| value(values, *argument).map(|value| value.visible.clone()))
                    .collect::<Result<_, _>>()?,
            })),
            MirInstructionKind::ErrorMake {
                error,
                case,
                arguments,
            } => Ok(RuntimeValue::visible(MirValue::Error {
                error: *error,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| value(values, *argument).map(|value| value.visible.clone()))
                    .collect::<Result<_, _>>()?,
            })),
            MirInstructionKind::InterfaceUpcast { value: base, .. } => {
                Ok(value(values, *base)?.clone())
            }
            MirInstructionKind::CheckedDowncast {
                value: base,
                target_type,
                target_class,
                ..
            } => {
                let candidate = value(values, *base)?;
                match &candidate.visible {
                    MirValue::Class(class)
                        if class_is_or_descends_from(
                            self.mir,
                            self.arena,
                            class.definition(),
                            *target_class,
                            *target_type,
                        ) =>
                    {
                        Ok(candidate.clone())
                    }
                    MirValue::Class(_) => Ok(RuntimeValue::visible(MirValue::Nil)),
                    _ => Err(ExecutionError::TypeMismatch),
                }
            }
            _ => return Ok(None),
        }?;
        Ok(Some(result))
    }

    fn allocate_iteration_session(
        &mut self,
        iterator_type: TypeId,
        source: RuntimeValue,
    ) -> Result<RuntimeValue, ExecutionError> {
        let (expected_length, range_current) = match &source.visible {
            MirValue::Array(elements) => (elements.len(), None),
            MirValue::List(elements) => (elements.len(), None),
            MirValue::Table(entries) => (entries.len(), None),
            MirValue::Range { first, .. } => (0, Some(*first)),
            _ => return Err(ExecutionError::TypeMismatch),
        };
        let reference_slots = source
            .reference
            .map(|_| vec![ObjectSlot::new(0)])
            .unwrap_or_default();
        let object_map = ObjectMap::new(u32::from(source.reference.is_some()), reference_slots)
            .map_err(|_| ExecutionError::InvalidControlFlow)?;
        let reference = self
            .runtime
            .allocate_object(&ObjectAllocationRequest::new(
                RuntimeTypeId::new(iterator_type.raw()),
                AllocationClass::NurseryEligible,
                object_map,
            ))
            .map_err(ExecutionError::Runtime)?;
        let symbol = self.fresh_private_symbol();
        self.private_values.insert(
            symbol,
            PrivateValue::Iterator {
                source,
                expected_length,
                position: 0,
                range_current,
                range_started: false,
            },
        );
        Ok(RuntimeValue::managed(MirValue::Function(symbol), reference))
    }

    fn advance_iteration_session(
        &mut self,
        iteration_type: TypeId,
        iterator: &RuntimeValue,
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<RuntimeValue, ExecutionError> {
        let MirValue::Function(symbol) = iterator.visible else {
            return Err(ExecutionError::TypeMismatch);
        };
        let (source, expected_length, position, range_current, range_started) =
            match self.private_values.get(&symbol) {
                Some(PrivateValue::Iterator {
                    source,
                    expected_length,
                    position,
                    range_current,
                    range_started,
                }) => (
                    source.clone(),
                    *expected_length,
                    *position,
                    *range_current,
                    *range_started,
                ),
                _ => return Err(ExecutionError::TypeMismatch),
            };
        let current = source.reference.and_then(|owner| {
            values
                .values()
                .find(|candidate| candidate.reference == Some(owner))
                .cloned()
        });
        let current = current.as_ref().unwrap_or(&source);
        let (length, item, next_range) = match &current.visible {
            MirValue::Array(elements) => (elements.len(), elements.get(position).cloned(), None),
            MirValue::List(elements) => (elements.len(), elements.get(position).cloned(), None),
            MirValue::Table(entries) => (
                entries.len(),
                entries
                    .get(position)
                    .map(|(key, value)| MirValue::Tuple(vec![key.clone(), value.clone()])),
                None,
            ),
            MirValue::Range { last, step, .. } => {
                let Some(current) = range_current else {
                    return self.iteration_result(iteration_type, None);
                };
                let next = if range_started {
                    current.checked_add(*step).map_err(|error| match error {
                        pop_types::NumericError::KindMismatch => ExecutionError::TypeMismatch,
                        _ => ExecutionError::Runtime(
                            self.runtime
                                .raise_trap(Trap::new(TrapKind::IntegerOverflow)),
                        ),
                    })?
                } else {
                    current
                };
                let ordering = next
                    .compare(*last)
                    .map_err(|_| ExecutionError::TypeMismatch)?;
                let positive = step.signed().map_or_else(
                    || step.unsigned().is_some_and(|value| value > 0),
                    |value| value > 0,
                );
                let in_range = if positive {
                    !ordering.is_gt()
                } else {
                    !ordering.is_lt()
                };
                if !in_range {
                    if let Some(PrivateValue::Iterator { range_current, .. }) =
                        self.private_values.get_mut(&symbol)
                    {
                        *range_current = None;
                    }
                    return self.iteration_result(iteration_type, None);
                }
                let following = (!ordering.is_eq()).then_some(next);
                (0, Some(MirValue::Integer(next)), following)
            }
            _ => return Err(ExecutionError::TypeMismatch),
        };
        if !matches!(current.visible, MirValue::Range { .. }) && length != expected_length {
            return Err(ExecutionError::Runtime(
                self.runtime
                    .raise_trap(Trap::new(TrapKind::ConcurrentModification)),
            ));
        }
        if item.is_some()
            && let Some(PrivateValue::Iterator {
                position,
                range_current,
                range_started,
                ..
            }) = self.private_values.get_mut(&symbol)
        {
            if matches!(current.visible, MirValue::Range { .. }) {
                *range_current = next_range;
                *range_started = true;
            } else {
                *position = position.saturating_add(1);
            }
        }
        self.iteration_result(iteration_type, item)
    }

    fn iteration_result(
        &self,
        iteration_type: TypeId,
        item: Option<MirValue>,
    ) -> Result<RuntimeValue, ExecutionError> {
        let definition = match self.arena.get(iteration_type) {
            Some(SemanticType::Builtin { definition, .. }) => *definition,
            _ => return Err(ExecutionError::TypeMismatch),
        };
        Ok(RuntimeValue::visible(MirValue::Iteration {
            definition,
            case: pop_foundation::IterationCaseId::from_raw(u32::from(item.is_none())),
            arguments: item.into_iter().collect(),
        }))
    }

    fn execute_direct_call(
        &mut self,
        function: SymbolId,
        arguments: &[ValueId],
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let arguments = evaluated_arguments(arguments, values)?;
        self.call(function, &arguments)
    }

    fn execute_method_call(
        &mut self,
        method: pop_foundation::MethodId,
        arguments: &[ValueId],
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let arguments = evaluated_arguments(arguments, values)?;
        let function = self
            .mir
            .methods()
            .iter()
            .find(|candidate| candidate.method() == method)
            .ok_or(ExecutionError::InvalidControlFlow)?
            .function();
        if function.parameters().len() != arguments.len() {
            return Err(ExecutionError::WrongArity);
        }
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(ExecutionError::CallDepthLimit)?;
        if self.depth > self.limits.maximum_call_depth {
            return Err(ExecutionError::CallDepthLimit);
        }
        let returned = self.execute(
            function.parameters(),
            function.results(),
            function.blocks(),
            &arguments,
            None,
        );
        self.depth -= 1;
        returned
    }

    fn execute_indirect_call(
        &mut self,
        callee: ValueId,
        arguments: &[ValueId],
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let callee = value(values, callee)?.clone();
        let arguments = evaluated_arguments(arguments, values)?;
        self.execute_indirect_value(&callee, &arguments)
    }

    fn execute_scoped_borrow_call(
        &mut self,
        owner: SymbolId,
        function: NestedFunctionId,
        captures: &[pop_mir::MirClosureCapture],
        arguments: &[ValueId],
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        if captures.iter().any(|capture| capture.self_reference()) {
            return Err(ExecutionError::InvalidControlFlow);
        }
        let nested = self
            .mir
            .nested_functions()
            .iter()
            .find(|candidate| candidate.owner() == owner && candidate.function() == function)
            .ok_or(ExecutionError::InvalidControlFlow)?;
        let capture_values = captures
            .iter()
            .map(|capture| value(values, capture.value()).cloned())
            .collect::<Result<Vec<_>, _>>()?;
        let arguments = evaluated_arguments(arguments, values)?;
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(ExecutionError::CallDepthLimit)?;
        if self.depth > self.limits.maximum_call_depth {
            return Err(ExecutionError::CallDepthLimit);
        }
        let result = self.execute(
            nested.parameters(),
            nested.results(),
            nested.blocks(),
            &arguments,
            Some(Rc::new(RefCell::new(capture_values))),
        );
        self.depth -= 1;
        result
    }

    fn execute_callback_pair_call(
        &mut self,
        owner: SymbolId,
        function: NestedFunctionId,
        captures: &[pop_mir::MirClosureCapture],
        arguments: &[RuntimeValue],
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        if captures.iter().any(|capture| capture.self_reference()) {
            return Err(ExecutionError::InvalidControlFlow);
        }
        let nested = self
            .mir
            .nested_functions()
            .iter()
            .find(|candidate| candidate.owner() == owner && candidate.function() == function)
            .ok_or(ExecutionError::InvalidControlFlow)?;
        let capture_values = captures
            .iter()
            .map(|capture| value(values, capture.value()).cloned())
            .collect::<Result<Vec<_>, _>>()?;
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(ExecutionError::CallDepthLimit)?;
        if self.depth > self.limits.maximum_call_depth {
            return Err(ExecutionError::CallDepthLimit);
        }
        let result = self.execute(
            nested.parameters(),
            nested.results(),
            nested.blocks(),
            arguments,
            Some(Rc::new(RefCell::new(capture_values))),
        );
        self.depth -= 1;
        result
    }

    fn interpreter_callback_target(
        &self,
        callback: &RuntimeValue,
        expected_owner: SymbolId,
        expected_function: NestedFunctionId,
    ) -> Result<InterpreterCallbackTarget, ExecutionError> {
        let MirValue::Function(symbol) = callback.visible else {
            return Err(ExecutionError::TypeMismatch);
        };
        match self.private_values.get(&symbol) {
            Some(PrivateValue::Closure {
                owner,
                function,
                captures,
            }) if *owner == expected_owner && *function == expected_function => {
                Ok(InterpreterCallbackTarget::Closure {
                    owner: *owner,
                    function: *function,
                    captures: captures.clone(),
                })
            }
            _ => Err(ExecutionError::InvalidControlFlow),
        }
    }

    fn execute_callback_target(
        &mut self,
        target: &InterpreterCallbackTarget,
        arguments: &[RuntimeValue],
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let InterpreterCallbackTarget::Closure {
            owner,
            function,
            captures,
        } = target;
        let nested = self
            .mir
            .nested_functions()
            .iter()
            .find(|candidate| candidate.owner() == *owner && candidate.function() == *function)
            .ok_or(ExecutionError::InvalidControlFlow)?;
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(ExecutionError::CallDepthLimit)?;
        if self.depth > self.limits.maximum_call_depth {
            return Err(ExecutionError::CallDepthLimit);
        }
        let returned = self.execute(
            nested.parameters(),
            nested.results(),
            nested.blocks(),
            arguments,
            Some(captures.clone()),
        );
        self.depth -= 1;
        returned
    }

    fn invoke_ffi_callback(
        &mut self,
        function: &MirValue,
        context: &MirValue,
        arguments: &[MirValue],
    ) -> Result<Vec<MirValue>, ExecutionError> {
        let MirValue::FfiFunction(raw_site) = function else {
            return Err(ExecutionError::TypeMismatch);
        };
        let context = ffi_pointer(context)?;
        let site = FfiCallbackSiteId::new(*raw_site).ok_or(ExecutionError::TypeMismatch)?;
        let state = self
            .ffi_callbacks
            .values()
            .find(|state| state.site == site && state.registration.context() == context)
            .cloned()
            .filter(|state| !state.closed)
            .ok_or(ExecutionError::UnsupportedFfiCallback {
                function: *raw_site,
                context,
            })?;
        let InterpreterCallbackTarget::Closure {
            owner,
            function: callback,
            ..
        } = &state.target;
        let nested = self
            .mir
            .nested_functions()
            .iter()
            .find(|candidate| candidate.owner() == *owner && candidate.function() == *callback)
            .ok_or(ExecutionError::InvalidControlFlow)?;
        if nested.parameters().len() != arguments.len() || nested.results().len() != 1 {
            return Err(ExecutionError::WrongArity);
        }
        let mut context_parameters = 0_u8;
        for (parameter, argument) in nested.parameters().iter().zip(arguments) {
            let is_context = matches!(
                self.arena.get(*parameter),
                Some(SemanticType::Builtin { definition, arguments })
                    if *definition == FFI_CALLBACK_CONTEXT_TYPE_ID && arguments.is_empty()
            );
            if is_context {
                context_parameters = context_parameters.saturating_add(1);
                if argument != &MirValue::FfiPointer(context) {
                    return Err(ExecutionError::TypeMismatch);
                }
            }
            if !callback_abi_value_matches(self.mir, self.arena, *parameter, argument)? {
                return Err(ExecutionError::TypeMismatch);
            }
        }
        if context_parameters != 1 {
            return Err(ExecutionError::InvalidControlFlow);
        }
        let entry = self
            .runtime
            .ffi_callback_enter(context, site)
            .map_err(ExecutionError::Runtime)?;
        if entry.environment() != Some(state.environment) {
            let _ = self.runtime.ffi_callback_leave(entry.transition());
            return Err(self.runtime_invariant());
        }
        let runtime_arguments = arguments
            .iter()
            .cloned()
            .map(RuntimeValue::visible)
            .collect::<Vec<_>>();
        let invocation = self.execute_callback_target(&state.target, &runtime_arguments);
        self.runtime
            .ffi_callback_leave(entry.transition())
            .map_err(ExecutionError::Runtime)?;
        let returned = invocation?;
        let [returned] = returned.as_slice() else {
            return Err(ExecutionError::WrongArity);
        };
        if !callback_abi_value_matches(
            self.mir,
            self.arena,
            nested.results()[0],
            &returned.visible,
        )? {
            return Err(ExecutionError::TypeMismatch);
        }
        Ok(vec![returned.visible.clone()])
    }

    fn execute_indirect_value(
        &mut self,
        callee: &RuntimeValue,
        arguments: &[RuntimeValue],
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let MirValue::Function(function) = &callee.visible else {
            return Err(ExecutionError::TypeMismatch);
        };
        let closure = match self.private_values.get(function) {
            Some(PrivateValue::Closure {
                owner,
                function,
                captures,
            }) => Some((*owner, *function, captures.clone())),
            _ => None,
        };
        if let Some((owner, function, captures)) = closure {
            let nested = self
                .mir
                .nested_functions()
                .iter()
                .find(|candidate| candidate.owner() == owner && candidate.function() == function)
                .ok_or(ExecutionError::InvalidControlFlow)?;
            self.depth = self
                .depth
                .checked_add(1)
                .ok_or(ExecutionError::CallDepthLimit)?;
            if self.depth > self.limits.maximum_call_depth {
                return Err(ExecutionError::CallDepthLimit);
            }
            let result = self.execute(
                nested.parameters(),
                nested.results(),
                nested.blocks(),
                arguments,
                Some(captures),
            );
            self.depth -= 1;
            result
        } else {
            self.call(*function, arguments)
        }
    }

    fn fresh_private_symbol(&mut self) -> SymbolId {
        let symbol = SymbolId::from_raw(self.next_private_value);
        self.next_private_value = self.next_private_value.saturating_sub(1);
        symbol
    }

    fn assign_block_arguments(
        blocks: &[pop_mir::MirBlock],
        target: pop_foundation::BlockId,
        arguments: &[ValueId],
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<(), ExecutionError> {
        let target = blocks
            .get(target.raw() as usize)
            .ok_or(ExecutionError::InvalidControlFlow)?;
        if target.arguments().len() != arguments.len() {
            return Err(ExecutionError::WrongArity);
        }
        let incoming: Result<Vec<_>, _> = arguments
            .iter()
            .map(|argument| value(values, *argument).cloned())
            .collect();
        for (parameter, incoming) in target.arguments().iter().zip(incoming?) {
            values.insert(parameter.value(), incoming);
        }
        Ok(())
    }

    fn assign_runtime_block_arguments(
        blocks: &[pop_mir::MirBlock],
        target: pop_foundation::BlockId,
        arguments: &[MirValue],
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<(), ExecutionError> {
        let target = blocks
            .get(target.raw() as usize)
            .ok_or(ExecutionError::InvalidControlFlow)?;
        if target.arguments().len() != arguments.len() {
            return Err(ExecutionError::WrongArity);
        }
        for (parameter, argument) in target.arguments().iter().zip(arguments) {
            values.insert(parameter.value(), RuntimeValue::visible(argument.clone()));
        }
        Ok(())
    }

    fn step(&mut self) -> Result<(), ExecutionError> {
        self.steps = self.steps.checked_add(1).ok_or(ExecutionError::StepLimit)?;
        if self.steps > self.limits.maximum_steps {
            Err(ExecutionError::StepLimit)
        } else {
            Ok(())
        }
    }
}

fn checked_view_range(owner_length: usize, start: i64, length: i64) -> Option<(usize, usize)> {
    let start = start
        .checked_sub(1)
        .and_then(|value| usize::try_from(value).ok())?;
    let length = usize::try_from(length).ok()?;
    let end = start.checked_add(length)?;
    if end > owner_length || (length == 0 && start > owner_length) {
        return None;
    }
    if length != 0 && start >= owner_length {
        return None;
    }
    Some((start, length))
}

fn install_published_relocations(
    roots: &[ValueId],
    publication: &RootPublication,
    values: &mut BTreeMap<ValueId, RuntimeValue>,
) -> Result<(), ExecutionError> {
    for (root, (_, relocated)) in roots.iter().copied().zip(publication.root_values()) {
        let previous = value(values, root)?.reference;
        if previous.is_some() != relocated.is_some() {
            return Err(ExecutionError::Runtime(RuntimeFailure::runtime_invariant()));
        }
        for candidate in values.values_mut() {
            if candidate.reference == previous {
                candidate.install_relocated_reference(relocated)?;
            }
        }
    }
    Ok(())
}

fn scalar_byte_offset(text: &str, scalar_index: usize) -> Option<usize> {
    if scalar_index == text.chars().count() {
        return Some(text.len());
    }
    text.char_indices()
        .nth(scalar_index)
        .map(|(offset, _)| offset)
}

fn view_text(view: &MirViewValue) -> Result<&str, ExecutionError> {
    let MirViewLenderValue::Text(text) = &view.lender else {
        return Err(ExecutionError::TypeMismatch);
    };
    let end = view
        .byte_offset
        .checked_add(view.byte_length)
        .filter(|end| *end <= text.len())
        .ok_or(ExecutionError::InvalidControlFlow)?;
    text.get(view.byte_offset..end)
        .ok_or(ExecutionError::InvalidControlFlow)
}

fn view_bytes_reference(view: &MirViewValue) -> Result<ManagedReference, ExecutionError> {
    match &view.lender {
        MirViewLenderValue::Bytes(reference) => Ok(*reference),
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn class_definition(bubble: &MirBubble, class: ClassId) -> Option<SymbolIdentity> {
    bubble
        .declarations()
        .iter()
        .find_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Class(candidate) if candidate.class() == class => {
                Some(candidate.definition())
            }
            _ => None,
        })
        .or_else(|| {
            bubble
                .nominal_references()
                .classes()
                .iter()
                .find(|reference| reference.class() == class)
                .map(|reference| reference.identity().definition())
        })
}

fn canonical_class_identity(
    bubble: &MirBubble,
    arena: &TypeArena,
    class: ClassId,
    type_id: TypeId,
) -> Option<pop_types::CanonicalNominalIdentity> {
    if let Some(reference) = bubble
        .nominal_references()
        .classes()
        .iter()
        .find(|reference| reference.class() == class && reference.type_id() == type_id)
    {
        return Some(reference.identity().canonical().clone());
    }
    let definition = class_definition(bubble, class)?;
    let SemanticType::Class {
        class: found,
        arguments,
    } = arena.get(type_id)?
    else {
        return None;
    };
    if *found != class {
        return None;
    }
    Some(pop_types::CanonicalNominalIdentity::new(
        definition,
        arguments
            .iter()
            .map(|argument| canonical_type_identity(bubble, arena, *argument))
            .collect::<Option<Vec<_>>>()?,
    ))
}

fn canonical_type_identity(
    bubble: &MirBubble,
    arena: &TypeArena,
    type_id: TypeId,
) -> Option<pop_types::CanonicalTypeIdentity> {
    use pop_types::CanonicalTypeIdentity as Canonical;
    Some(match arena.get(type_id)? {
        SemanticType::Primitive(primitive) => Canonical::Primitive(*primitive),
        SemanticType::Record(_) => {
            let declaration = bubble.declarations().iter().find(|declaration| {
                matches!(declaration.kind(), MirDeclarationKind::Record(record)
                    if record.type_id() == type_id)
            })?;
            Canonical::Record(SymbolIdentity::new(bubble.bubble(), declaration.symbol()))
        }
        SemanticType::Class { class, .. } => {
            Canonical::Class(canonical_class_identity(bubble, arena, *class, type_id)?)
        }
        SemanticType::Interface {
            interface,
            arguments,
        } => {
            if let Some(reference) =
                bubble
                    .nominal_references()
                    .interfaces()
                    .iter()
                    .find(|reference| {
                        reference.interface() == *interface && reference.type_id() == type_id
                    })
            {
                Canonical::Interface(reference.identity().canonical().clone())
            } else {
                let declaration = bubble.declarations().iter().find(|declaration| {
                    matches!(declaration.kind(), MirDeclarationKind::Interface(candidate)
                        if candidate.interface() == *interface)
                })?;
                Canonical::Interface(pop_types::CanonicalNominalIdentity::new(
                    SymbolIdentity::new(bubble.bubble(), declaration.symbol()),
                    arguments
                        .iter()
                        .map(|argument| canonical_type_identity(bubble, arena, *argument))
                        .collect::<Option<Vec<_>>>()?,
                ))
            }
        }
        SemanticType::Tuple(elements) => Canonical::Tuple(
            elements
                .iter()
                .map(|element| canonical_type_identity(bubble, arena, *element))
                .collect::<Option<Vec<_>>>()?,
        ),
        SemanticType::Function {
            is_async,
            parameters,
            results,
            effects,
            lifetime_summary,
        } => Canonical::Function {
            is_async: *is_async,
            parameters: parameters
                .iter()
                .map(|parameter| canonical_type_identity(bubble, arena, *parameter))
                .collect::<Option<Vec<_>>>()?,
            results: results
                .iter()
                .map(|result| canonical_type_identity(bubble, arena, *result))
                .collect::<Option<Vec<_>>>()?,
            effects: *effects,
            lifetime_summary: lifetime_summary.clone(),
        },
        SemanticType::Array(element) => {
            Canonical::Array(Box::new(canonical_type_identity(bubble, arena, *element)?))
        }
        SemanticType::Table { key, value } => Canonical::Table {
            key: Box::new(canonical_type_identity(bubble, arena, *key)?),
            value: Box::new(canonical_type_identity(bubble, arena, *value)?),
        },
        SemanticType::Optional(element) => {
            Canonical::Optional(Box::new(canonical_type_identity(bubble, arena, *element)?))
        }
        SemanticType::Builtin {
            definition,
            arguments,
        } => Canonical::Builtin {
            definition: *definition,
            arguments: arguments
                .iter()
                .map(|argument| canonical_type_identity(bubble, arena, *argument))
                .collect::<Option<Vec<_>>>()?,
        },
        SemanticType::Union(elements) => Canonical::Union(
            elements
                .iter()
                .map(|element| canonical_type_identity(bubble, arena, *element))
                .collect::<Option<Vec<_>>>()?,
        ),
        SemanticType::TaggedUnion { .. }
        | SemanticType::ErrorUnion { .. }
        | SemanticType::Enum { .. }
        | SemanticType::Attribute { .. }
        | SemanticType::TypeParameter(_)
        | SemanticType::Opaque(_)
        | SemanticType::Error => return None,
    })
}

fn class_is_or_descends_from(
    bubble: &MirBubble,
    arena: &TypeArena,
    concrete: &pop_types::CanonicalNominalIdentity,
    target: ClassId,
    target_type: TypeId,
) -> bool {
    let Some(target) = canonical_class_identity(bubble, arena, target, target_type) else {
        return false;
    };
    let mut classes = BTreeMap::new();
    for declaration in bubble.declarations() {
        let MirDeclarationKind::Class(class) = declaration.kind() else {
            continue;
        };
        let Some(identity) =
            canonical_class_identity(bubble, arena, class.class(), class.type_id())
        else {
            continue;
        };
        let base = class.base().and_then(|base| {
            bubble
                .declarations()
                .iter()
                .find_map(|declaration| match declaration.kind() {
                    MirDeclarationKind::Class(base_class) if base_class.class() == base => {
                        canonical_class_identity(bubble, arena, base, base_class.type_id())
                    }
                    _ => None,
                })
        });
        classes.insert(identity, base);
    }
    for reference in bubble.nominal_references().classes() {
        let base = reference
            .base()
            .zip(reference.base_type())
            .and_then(|(base, base_type)| {
                bubble
                    .nominal_references()
                    .classes()
                    .iter()
                    .find(|candidate| candidate.class() == base && candidate.type_id() == base_type)
                    .map(|candidate| candidate.identity().canonical().clone())
            });
        classes.insert(reference.identity().canonical().clone(), base);
    }
    let mut current = concrete.clone();
    let mut visited = BTreeSet::new();
    while visited.insert(current.clone()) {
        if current == target {
            return true;
        }
        let Some(base) = classes.get(&current).cloned().flatten() else {
            return false;
        };
        current = base;
    }
    false
}

impl<R: RuntimeAdapter> FfiCallbackInvoker for Engine<'_, '_, R> {
    fn invoke(
        &mut self,
        function: &MirValue,
        context: &MirValue,
        arguments: &[MirValue],
    ) -> Result<Vec<MirValue>, ExecutionError> {
        self.invoke_ffi_callback(function, context, arguments)
    }
}
