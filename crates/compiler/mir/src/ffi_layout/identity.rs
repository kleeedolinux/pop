use std::collections::BTreeMap;
use std::fmt::Write;

use super::{MirFfiLayout, MirFfiLayoutError, MirFfiValueClass};
use pop_runtime_interface::FfiAbiLayoutId;
use pop_target::TargetSpec;
use pop_types::{PrimitiveType, SemanticType, TypeArena, embedded_bootstrap_schema};

pub(super) fn canonicalize_identities(
    entries: Vec<MirFfiLayout>,
    types: &TypeArena,
    target: &TargetSpec,
    fingerprint: &impl Fn(&[u8]) -> String,
) -> Result<Vec<MirFfiLayout>, MirFfiLayoutError> {
    let by_id = entries
        .iter()
        .map(|entry| (entry.id, entry))
        .collect::<BTreeMap<_, _>>();
    let mut identities = BTreeMap::new();
    for entry in &entries {
        identity(
            entry.id,
            &by_id,
            &mut identities,
            types,
            target,
            fingerprint,
        )?;
    }
    let remap = identities
        .iter()
        .map(|(provisional, identity)| (*provisional, identity.id))
        .collect::<BTreeMap<_, _>>();
    let mut canonical = entries
        .into_iter()
        .map(|mut entry| {
            let identity = identities
                .remove(&entry.id)
                .expect("every validated layout has a canonical identity");
            entry.id = identity.id;
            entry.descriptor = identity.descriptor;
            entry.fingerprint = identity.fingerprint;
            if let MirFfiValueClass::Record(fields) = &mut entry.value_class {
                for field in fields {
                    field.layout = *remap
                        .get(&field.layout)
                        .expect("every record child was canonicalized");
                }
            }
            entry
        })
        .collect::<Vec<_>>();
    canonical.sort_by_key(MirFfiLayout::id);
    for pair in canonical.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(MirFfiLayoutError::CompactIdentityCollision(pair[0].id));
        }
    }
    Ok(canonical)
}

struct CanonicalIdentity {
    id: FfiAbiLayoutId,
    descriptor: String,
    fingerprint: String,
}

fn identity(
    id: FfiAbiLayoutId,
    entries: &BTreeMap<FfiAbiLayoutId, &MirFfiLayout>,
    identities: &mut BTreeMap<FfiAbiLayoutId, CanonicalIdentity>,
    types: &TypeArena,
    target: &TargetSpec,
    fingerprint: &impl Fn(&[u8]) -> String,
) -> Result<(), MirFfiLayoutError> {
    if identities.contains_key(&id) {
        return Ok(());
    }
    let entry = entries
        .get(&id)
        .copied()
        .ok_or(MirFfiLayoutError::MissingFieldLayout(id))?;
    if let MirFfiValueClass::Record(fields) = entry.value_class() {
        for field in fields {
            identity(
                field.layout(),
                entries,
                identities,
                types,
                target,
                fingerprint,
            )?;
        }
    }
    let descriptor = descriptor(entry, entries, identities, types, target)?;
    let fingerprint = fingerprint(descriptor.as_bytes());
    if fingerprint.len() != 64
        || !fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(MirFfiLayoutError::InvalidFingerprint(entry.id()));
    }
    let raw = u64::from_str_radix(&fingerprint[..16], 16)
        .map_err(|_| MirFfiLayoutError::InvalidFingerprint(entry.id()))?;
    let canonical =
        FfiAbiLayoutId::new(raw).ok_or(MirFfiLayoutError::ZeroCompactIdentity(entry.id()))?;
    identities.insert(
        id,
        CanonicalIdentity {
            id: canonical,
            descriptor,
            fingerprint,
        },
    );
    Ok(())
}

fn descriptor(
    entry: &MirFfiLayout,
    entries: &BTreeMap<FfiAbiLayoutId, &MirFfiLayout>,
    identities: &BTreeMap<FfiAbiLayoutId, CanonicalIdentity>,
    types: &TypeArena,
    target: &TargetSpec,
) -> Result<String, MirFfiLayoutError> {
    if let MirFfiValueClass::Record(fields) = entry.value_class() {
        let Some(SemanticType::Record(semantic_fields)) = types.get(entry.element()) else {
            return Err(MirFfiLayoutError::UnstableTypeIdentity(entry.id()));
        };
        let mut output = format!(
            "{{\"schemaVersion\":1,\"target\":\"{}\",\"abi\":\"{}\",\"size\":{},\"alignment\":{},\"fields\":[",
            json_escape(target.triple()),
            abi_name(entry.abi()),
            entry.size(),
            entry.alignment()
        );
        for (index, field) in fields.iter().enumerate() {
            if index != 0 {
                output.push(',');
            }
            let name = semantic_fields
                .get(field.source_index() as usize)
                .map(|(name, _)| name)
                .ok_or(MirFfiLayoutError::UnstableTypeIdentity(entry.id()))?;
            let child = entries
                .get(&field.layout())
                .copied()
                .ok_or(MirFfiLayoutError::MissingFieldLayout(entry.id()))?;
            let child_identity = identities
                .get(&field.layout())
                .ok_or(MirFfiLayoutError::MissingFieldLayout(entry.id()))?;
            let abi_type = match child.value_class() {
                MirFfiValueClass::Record(_) => {
                    format!("layout:{}", child_identity.fingerprint)
                }
                _ => semantic_name(child.element(), types, entry.id())?,
            };
            let _ = write!(
                output,
                "{{\"name\":\"{}\",\"abiType\":\"{}\",\"offset\":{},\"size\":{},\"alignment\":{}}}",
                json_escape(name),
                json_escape(&abi_type),
                field.offset(),
                child.size(),
                child.alignment()
            );
        }
        output.push_str("]}");
        return Ok(output);
    }
    Ok(format!(
        "{{\"schemaVersion\":1,\"target\":\"{}\",\"abi\":\"{}\",\"abiType\":\"{}\",\"size\":{},\"alignment\":{}}}",
        json_escape(target.triple()),
        abi_name(entry.abi()),
        json_escape(&semantic_name(entry.element(), types, entry.id())?),
        entry.size(),
        entry.alignment()
    ))
}

const fn abi_name(abi: pop_types::ForeignAbi) -> &'static str {
    match abi {
        pop_types::ForeignAbi::C => "C",
        pop_types::ForeignAbi::System => "System",
        pop_types::ForeignAbi::CUnwind => "CUnwind",
    }
}

fn semantic_name(
    type_id: pop_foundation::TypeId,
    types: &TypeArena,
    layout: FfiAbiLayoutId,
) -> Result<String, MirFfiLayoutError> {
    match types.get(type_id) {
        Some(SemanticType::Primitive(primitive)) => primitive_name(*primitive)
            .map(str::to_owned)
            .ok_or(MirFfiLayoutError::UnstableTypeIdentity(layout)),
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) => {
            let schema = embedded_bootstrap_schema()
                .map_err(|_| MirFfiLayoutError::UnstableTypeIdentity(layout))?;
            let name = schema
                .type_by_id(*definition)
                .map(|entry| entry.source_name())
                .ok_or(MirFfiLayoutError::UnstableTypeIdentity(layout))?;
            if arguments.is_empty() {
                return Ok(name.to_owned());
            }
            let arguments = arguments
                .iter()
                .map(|argument| semantic_name(*argument, types, layout))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{name}<{}>", arguments.join(",")))
        }
        Some(SemanticType::Function {
            is_async: false,
            parameters,
            results,
            ..
        }) => {
            let parameters = semantic_names(parameters, types, layout)?;
            let results = semantic_names(results, types, layout)?;
            Ok(format!("function({parameters})->({results})"))
        }
        _ => Err(MirFfiLayoutError::UnstableTypeIdentity(layout)),
    }
}

fn semantic_names(
    type_ids: &[pop_foundation::TypeId],
    types: &TypeArena,
    layout: FfiAbiLayoutId,
) -> Result<String, MirFfiLayoutError> {
    type_ids
        .iter()
        .map(|type_id| semantic_name(*type_id, types, layout))
        .collect::<Result<Vec<_>, _>>()
        .map(|names| names.join(","))
}

const fn primitive_name(primitive: PrimitiveType) -> Option<&'static str> {
    Some(match primitive {
        PrimitiveType::Integer(kind) => match kind {
            pop_types::IntegerKind::Int8 => "Int8",
            pop_types::IntegerKind::Int16 => "Int16",
            pop_types::IntegerKind::Int32 => "Int32",
            pop_types::IntegerKind::Int64 => "Int64",
            pop_types::IntegerKind::UInt8 => "UInt8",
            pop_types::IntegerKind::UInt16 => "UInt16",
            pop_types::IntegerKind::UInt32 => "UInt32",
            pop_types::IntegerKind::UInt64 => "UInt64",
        },
        PrimitiveType::Float32 => "Float32",
        PrimitiveType::Float64 => "Float64",
        _ => return None,
    })
}

fn json_escape(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value <= '\u{1f}' => {
                let _ = write!(output, "\\u{:04x}", u32::from(value));
            }
            value => output.push(value),
        }
    }
    output
}
