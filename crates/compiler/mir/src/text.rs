use std::error::Error;
use std::fmt;

use pop_foundation::{
    BindingId, BlockId, BubbleId, BuiltinTypeId, CaptureId, ClassId, CleanupScopeId,
    CoroutineStateId, EnumCaseId, ErrorCaseId, ErrorId, FieldId, FileId, FunctionId, InterfaceId,
    InterfaceMethodId, IterationCaseId, IterationProtocolMethodId, MethodId, NamespaceId,
    NestedFunctionId, NominalInterfaceId, ResultCaseId, SourceSpan, StandardFunctionId, SymbolId,
    SymbolIdentity, TextRange, TextSize, TypeId, UnionCaseId, ValueId,
};
use pop_runtime_interface::{
    ArrayElementMap, ObjectMap, ObjectSlot, PanicKind, PanicPayload, RootSlot, SafePointId,
    StackMap, Trap, TrapKind, UnwindReason,
};
use pop_types::{FloatKind, FloatValue, IntegerKind, IntegerValue};

use super::{
    MirBlock, MirBlockArgument, MirBubble, MirBuiltinInterfaceImplementation,
    MirBuiltinInterfaceMethodImplementation, MirCancellationMode, MirCapture, MirCaptureMode,
    MirClassDeclaration, MirCleanupBlock, MirCleanupExitReason, MirClosureCapture, MirDeclaration,
    MirDeclarationKind, MirEffect, MirEffectSummary, MirEnumCase, MirEnumDeclaration, MirErrorCase,
    MirErrorDeclaration, MirErrorSwitchArm, MirField, MirFrameSlot, MirFunction,
    MirFunctionReference, MirInstruction, MirInstructionKind, MirInterfaceDeclaration,
    MirInterfaceImplementation, MirInterfaceMethod, MirInterfaceMethodImplementation, MirLiveFrame,
    MirMethod, MirNestedFunction, MirRecordDeclaration, MirSuspendOperation, MirTaskDispatch,
    MirTerminator, MirUnionCase, MirUnionDeclaration, MirUnionSwitchArm, MirUnwindAction,
    local_instruction_effects,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirParseError {
    line: usize,
    reason: &'static str,
}

impl MirParseError {
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for MirParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid MIR dump at line {}: {}",
            self.line, self.reason
        )
    }
}

impl Error for MirParseError {}

/// Parses the deterministic MIR test/interchange text produced by `MirBubble::dump`.
///
/// # Errors
///
/// Returns a line-addressed structural parse error. Semantic invalidity remains
/// the MIR verifier's responsibility.
pub fn parse_mir_dump(text: &str) -> Result<MirBubble, MirParseError> {
    let lines: Vec<_> = text.lines().collect();
    let header = lines.first().ok_or_else(|| error(1, "missing header"))?;
    let (bubble, namespace) = parse_header(header, 1)?;
    let dependencies = parse_dependencies(
        lines
            .get(1)
            .ok_or_else(|| error(2, "missing dependencies"))?,
        2,
    )?;
    let mut position = 2;
    let mut declarations = Vec::new();
    let mut functions = Vec::new();
    let mut methods = Vec::new();
    let mut nested_functions = Vec::new();
    let mut function_references = Vec::new();
    while position < lines.len() {
        if lines[position].starts_with("reference ")
            || lines[position].starts_with("async reference ")
        {
            function_references.push(parse_function_reference(lines[position], position + 1)?);
            position += 1;
        } else if lines[position].starts_with("type.") {
            declarations.push(parse_declaration(lines[position], position + 1)?);
            position += 1;
        } else if lines[position].starts_with("method ") {
            let components: Vec<_> = lines[position].split_whitespace().collect();
            if components.len() != 3 {
                return Err(error(position + 1, "malformed method header"));
            }
            let method = MethodId::from_raw(parse_prefixed(components[1], 'm', position + 1)?);
            let class = ClassId::from_raw(parse_prefixed(components[2], 'c', position + 1)?);
            let (function, next) = parse_function(&lines, position + 1)?;
            methods.push(MirMethod {
                method,
                class,
                function,
            });
            position = next;
        } else if lines[position].starts_with("nested ")
            || lines[position].starts_with("async nested ")
        {
            let (function, next) = parse_nested_function(&lines, position)?;
            nested_functions.push(function);
            position = next;
        } else {
            let (function, next) = parse_function(&lines, position)?;
            functions.push(function);
            position = next;
        }
    }
    Ok(MirBubble {
        bubble,
        namespace,
        dependencies,
        declarations,
        functions,
        methods,
        nested_functions,
        function_references,
    })
}

fn parse_function_reference(
    line: &str,
    number: usize,
) -> Result<MirFunctionReference, MirParseError> {
    let (is_async, line) = line
        .strip_prefix("async ")
        .map_or((false, line), |line| (true, line));
    let parts: Vec<_> = line.split_whitespace().collect();
    if parts.len() != 5 || parts[0] != "reference" {
        return Err(error(number, "malformed function reference"));
    }
    let identity = parts[1]
        .strip_prefix('b')
        .and_then(|identity| identity.split_once(":s"))
        .ok_or_else(|| error(number, "function reference identity"))?;
    let effects = parts[4]
        .strip_prefix("effects[")
        .and_then(|effects| effects.strip_suffix(']'))
        .ok_or_else(|| error(number, "function reference effects"))?;
    Ok(MirFunctionReference {
        identity: SymbolIdentity::new(
            BubbleId::from_raw(parse_u32(identity.0, number)?),
            SymbolId::from_raw(parse_u32(identity.1, number)?),
        ),
        is_async,
        parameters: parse_wrapped_types(parts[2], "params(", number)?,
        results: parse_wrapped_types(parts[3], "results(", number)?,
        effects: parse_effects(effects, number)?,
    })
}

fn parse_declaration(line: &str, number: usize) -> Result<MirDeclaration, MirParseError> {
    let components: Vec<_> = line.split_whitespace().collect();
    match components.as_slice() {
        ["type.record", symbol, type_id, "fields", fields] => Ok(MirDeclaration {
            symbol: SymbolId::from_raw(parse_prefixed(symbol, 's', number)?),
            kind: MirDeclarationKind::Record(MirRecordDeclaration {
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', number)?),
                fields: parse_declared_fields(fields, number)?,
            }),
        }),
        ["type.union", symbol, type_id, "cases", cases] => Ok(MirDeclaration {
            symbol: SymbolId::from_raw(parse_prefixed(symbol, 's', number)?),
            kind: MirDeclarationKind::Union(MirUnionDeclaration {
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', number)?),
                cases: parse_union_cases(cases, number)?,
            }),
        }),
        ["type.error", symbol, error_id, type_id, "cases", cases] => Ok(MirDeclaration {
            symbol: SymbolId::from_raw(parse_prefixed(symbol, 's', number)?),
            kind: MirDeclarationKind::Error(MirErrorDeclaration {
                error: ErrorId::from_raw(parse_prefixed(error_id, 'e', number)?),
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', number)?),
                cases: parse_error_cases(cases, number)?,
            }),
        }),
        ["type.enum", symbol, type_id, "cases", cases] => Ok(MirDeclaration {
            symbol: SymbolId::from_raw(parse_prefixed(symbol, 's', number)?),
            kind: MirDeclarationKind::Enum(MirEnumDeclaration {
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', number)?),
                cases: parse_enum_cases(cases, number)?,
            }),
        }),
        [
            "type.class",
            symbol,
            class,
            type_id,
            "fields",
            fields,
            "methods",
            methods,
            "implements",
            implementations,
            "implementsBuiltin",
            builtin_implementations,
        ] => Ok(MirDeclaration {
            symbol: SymbolId::from_raw(parse_prefixed(symbol, 's', number)?),
            kind: MirDeclarationKind::Class(MirClassDeclaration {
                class: ClassId::from_raw(parse_prefixed(class, 'c', number)?),
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', number)?),
                fields: parse_declared_fields(fields, number)?,
                methods: parse_method_ids(methods, number)?,
                interfaces: parse_interface_implementations(implementations, number)?,
                builtin_interfaces: parse_builtin_interface_implementations(
                    builtin_implementations,
                    number,
                )?,
            }),
        }),
        [
            "type.class",
            symbol,
            class,
            type_id,
            "fields",
            fields,
            "methods",
            methods,
            "implements",
            implementations,
        ] => Ok(MirDeclaration {
            symbol: SymbolId::from_raw(parse_prefixed(symbol, 's', number)?),
            kind: MirDeclarationKind::Class(MirClassDeclaration {
                class: ClassId::from_raw(parse_prefixed(class, 'c', number)?),
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', number)?),
                fields: parse_declared_fields(fields, number)?,
                methods: parse_method_ids(methods, number)?,
                interfaces: parse_interface_implementations(implementations, number)?,
                builtin_interfaces: Vec::new(),
            }),
        }),
        [
            "type.interface",
            symbol,
            interface,
            type_id,
            "methods",
            methods,
        ] => Ok(MirDeclaration {
            symbol: SymbolId::from_raw(parse_prefixed(symbol, 's', number)?),
            kind: MirDeclarationKind::Interface(MirInterfaceDeclaration {
                interface: InterfaceId::from_raw(parse_prefixed(interface, 'i', number)?),
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', number)?),
                methods: parse_interface_methods(methods, number)?,
            }),
        }),
        _ => Err(error(number, "malformed type declaration")),
    }
}

fn parse_interface_methods(
    text: &str,
    line: usize,
) -> Result<Vec<MirInterfaceMethod>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|method| {
            let (identity, signature) = method
                .split_once('(')
                .ok_or_else(|| error(line, "interface method"))?;
            let (method, slot) = identity
                .split_once('@')
                .ok_or_else(|| error(line, "interface method identity"))?;
            let (parameters, results) = signature
                .split_once(")->(")
                .ok_or_else(|| error(line, "interface method signature"))?;
            let results = results
                .strip_suffix(')')
                .ok_or_else(|| error(line, "interface method results"))?;
            Ok(MirInterfaceMethod {
                method: InterfaceMethodId::from_raw(parse_named_prefix(method, "im", line)?),
                slot: parse_u32(slot, line)?,
                parameters: parse_semicolon_types(parameters, line)?,
                results: parse_semicolon_types(results, line)?,
            })
        })
        .collect()
}

fn parse_interface_implementations(
    text: &str,
    line: usize,
) -> Result<Vec<MirInterfaceImplementation>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|implementation| {
            let (head, methods) = implementation
                .split_once('[')
                .ok_or_else(|| error(line, "interface implementation"))?;
            let methods = methods
                .strip_suffix(']')
                .ok_or_else(|| error(line, "interface implementation"))?;
            let (interface, type_id) = head
                .split_once(':')
                .ok_or_else(|| error(line, "interface implementation type"))?;
            let methods = if methods.is_empty() {
                Vec::new()
            } else {
                methods
                    .split(';')
                    .map(|mapping| {
                        let (identity, class_method) = mapping
                            .split_once('=')
                            .ok_or_else(|| error(line, "interface method mapping"))?;
                        let (method, slot) = identity
                            .split_once('@')
                            .ok_or_else(|| error(line, "interface method slot"))?;
                        Ok(MirInterfaceMethodImplementation {
                            interface_method: InterfaceMethodId::from_raw(parse_named_prefix(
                                method, "im", line,
                            )?),
                            slot: parse_u32(slot, line)?,
                            class_method: MethodId::from_raw(parse_prefixed(
                                class_method,
                                'm',
                                line,
                            )?),
                        })
                    })
                    .collect::<Result<_, _>>()?
            };
            Ok(MirInterfaceImplementation {
                interface: InterfaceId::from_raw(parse_prefixed(interface, 'i', line)?),
                interface_type: TypeId::from_raw(parse_prefixed(type_id, 't', line)?),
                methods,
            })
        })
        .collect()
}

fn parse_builtin_interface_implementations(
    text: &str,
    line: usize,
) -> Result<Vec<MirBuiltinInterfaceImplementation>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|implementation| {
            let (head, methods) = implementation
                .split_once('[')
                .ok_or_else(|| error(line, "built-in interface implementation"))?;
            let methods = methods
                .strip_suffix(']')
                .ok_or_else(|| error(line, "built-in interface implementation"))?;
            let (interface, type_id) = head
                .split_once(':')
                .ok_or_else(|| error(line, "built-in interface implementation type"))?;
            let methods = if methods.is_empty() {
                Vec::new()
            } else {
                methods
                    .split(';')
                    .map(|mapping| {
                        let (protocol_method, class_method) = mapping
                            .split_once('=')
                            .ok_or_else(|| error(line, "built-in interface method mapping"))?;
                        Ok(MirBuiltinInterfaceMethodImplementation {
                            protocol_method: IterationProtocolMethodId::from_raw(parse_hash(
                                protocol_method,
                                "iterationMethod#",
                                line,
                            )?),
                            class_method: MethodId::from_raw(parse_prefixed(
                                class_method,
                                'm',
                                line,
                            )?),
                        })
                    })
                    .collect::<Result<_, _>>()?
            };
            Ok(MirBuiltinInterfaceImplementation {
                interface: BuiltinTypeId::from_raw(parse_prefixed(interface, 'b', line)?),
                interface_type: TypeId::from_raw(parse_prefixed(type_id, 't', line)?),
                methods,
            })
        })
        .collect()
}

fn parse_semicolon_types(text: &str, line: usize) -> Result<Vec<TypeId>, MirParseError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(';')
        .map(|type_id| parse_prefixed(type_id, 't', line).map(TypeId::from_raw))
        .collect()
}

fn parse_declared_fields(text: &str, line: usize) -> Result<Vec<MirField>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|field| {
            let (field, type_id) = field
                .split_once(':')
                .ok_or_else(|| error(line, "malformed declared field"))?;
            Ok(MirField {
                field: FieldId::from_raw(parse_hash(field, "field#", line)?),
                field_type: TypeId::from_raw(parse_prefixed(type_id, 't', line)?),
            })
        })
        .collect()
}

fn parse_union_cases(text: &str, line: usize) -> Result<Vec<MirUnionCase>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|case| {
            let (case, parameters) = case
                .strip_suffix(')')
                .and_then(|case| case.split_once('('))
                .ok_or_else(|| error(line, "malformed declared union case"))?;
            let parameters = if parameters.is_empty() {
                Vec::new()
            } else {
                parameters
                    .split(';')
                    .map(|type_id| parse_prefixed(type_id, 't', line).map(TypeId::from_raw))
                    .collect::<Result<_, _>>()?
            };
            Ok(MirUnionCase {
                case: UnionCaseId::from_raw(parse_hash(case, "case#", line)?),
                parameters,
            })
        })
        .collect()
}

fn parse_error_cases(text: &str, line: usize) -> Result<Vec<MirErrorCase>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|case| {
            let (case, parameters) = case
                .strip_suffix(')')
                .and_then(|case| case.split_once('('))
                .ok_or_else(|| error(line, "malformed declared error case"))?;
            let parameters = if parameters.is_empty() {
                Vec::new()
            } else {
                parameters
                    .split(';')
                    .map(|type_id| parse_prefixed(type_id, 't', line).map(TypeId::from_raw))
                    .collect::<Result<_, _>>()?
            };
            Ok(MirErrorCase {
                case: ErrorCaseId::from_raw(parse_hash(case, "errorCase#", line)?),
                parameters,
            })
        })
        .collect()
}

fn parse_enum_cases(text: &str, line: usize) -> Result<Vec<MirEnumCase>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|case| {
            let (case, discriminant) = case
                .split_once('=')
                .ok_or_else(|| error(line, "malformed declared enum case"))?;
            Ok(MirEnumCase {
                case: EnumCaseId::from_raw(parse_hash(case, "case#", line)?),
                discriminant: parse_u32(discriminant, line)?,
            })
        })
        .collect()
}

fn parse_method_ids(text: &str, line: usize) -> Result<Vec<MethodId>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|method| parse_prefixed(method, 'm', line).map(MethodId::from_raw))
        .collect()
}

fn parse_header(line: &str, number: usize) -> Result<(BubbleId, NamespaceId), MirParseError> {
    let components: Vec<_> = line.split_whitespace().collect();
    if components.len() != 5 || components[..2] != ["mir", "bubble"] || components[3] != "namespace"
    {
        return Err(error(number, "malformed header"));
    }
    Ok((
        BubbleId::from_raw(parse_prefixed(components[2], 'b', number)?),
        NamespaceId::from_raw(parse_prefixed(components[4], 'n', number)?),
    ))
}

fn parse_dependencies(line: &str, number: usize) -> Result<Vec<BubbleId>, MirParseError> {
    let mut components = line.split_whitespace();
    if components.next() != Some("dependencies") {
        return Err(error(number, "missing dependencies header"));
    }
    components
        .map(|component| parse_prefixed(component, 'b', number).map(BubbleId::from_raw))
        .collect()
}

fn parse_function(lines: &[&str], start: usize) -> Result<(MirFunction, usize), MirParseError> {
    let number = start + 1;
    let (is_async, header) = lines[start]
        .strip_prefix("async ")
        .map_or((false, lines[start]), |header| (true, header));
    let rest = header
        .strip_prefix("function s")
        .ok_or_else(|| error(number, "expected function"))?;
    let (symbol, rest) = split_number(rest, number)?;
    let rest = rest
        .strip_prefix(" f")
        .ok_or_else(|| error(number, "missing FunctionId"))?;
    let (function, rest) = split_number(rest, number)?;
    let rest = rest
        .strip_prefix('(')
        .ok_or_else(|| error(number, "missing parameter list"))?;
    let (parameters, rest) = rest
        .split_once(") -> (")
        .ok_or_else(|| error(number, "malformed signature"))?;
    let (results, effects) = if let Some((results, effects)) = rest.split_once(") effects[") {
        let effects = effects
            .strip_suffix(']')
            .ok_or_else(|| error(number, "malformed effect summary"))?;
        (results, parse_effects(effects, number)?)
    } else {
        (
            rest.strip_suffix(')')
                .ok_or_else(|| error(number, "malformed result list"))?,
            MirEffectSummary::empty(),
        )
    };
    let parameters = parse_types(parameters, number)?;
    let results = parse_types(results, number)?;
    let mut position = start + 1;
    let mut blocks = Vec::new();
    while position < lines.len() && lines[position].starts_with("  b") {
        let (block, next) = parse_block(lines, position)?;
        blocks.push(block);
        position = next;
    }
    if blocks.is_empty() {
        return Err(error(number, "function has no blocks"));
    }
    Ok((
        MirFunction {
            function: FunctionId::from_raw(function),
            symbol: SymbolId::from_raw(symbol),
            is_async,
            parameters,
            results,
            effects,
            effects_explicit: true,
            blocks,
        },
        position,
    ))
}

fn parse_nested_function(
    lines: &[&str],
    start: usize,
) -> Result<(MirNestedFunction, usize), MirParseError> {
    let line = start + 1;
    let (is_async, header) = lines[start]
        .strip_prefix("async ")
        .map_or((false, lines[start]), |header| (true, header));
    let parts: Vec<_> = header.split_whitespace().collect();
    if parts.len() != 8 || parts[0] != "nested" || parts[3] != "captures" {
        return Err(error(line, "nested function header"));
    }
    let owner = SymbolId::from_raw(parse_prefixed(parts[1], 's', line)?);
    let function = NestedFunctionId::from_raw(parse_named_prefix(parts[2], "nf", line)?);
    let captures = parse_captures(parts[4], line)?;
    let parameters = parse_wrapped_types(parts[5], "params(", line)?;
    let results = parse_wrapped_types(parts[6], "results(", line)?;
    let effects = parts[7]
        .strip_prefix("effects[")
        .and_then(|effects| effects.strip_suffix(']'))
        .ok_or_else(|| error(line, "nested effects"))
        .and_then(|effects| parse_effects(effects, line))?;
    let mut position = start + 1;
    let mut blocks = Vec::new();
    while position < lines.len() && lines[position].starts_with("  b") {
        let (block, next) = parse_block(lines, position)?;
        blocks.push(block);
        position = next;
    }
    if blocks.is_empty() {
        return Err(error(line, "nested function has no blocks"));
    }
    Ok((
        MirNestedFunction {
            owner,
            function,
            is_async,
            captures,
            parameters,
            results,
            effects,
            effects_explicit: true,
            blocks,
        },
        position,
    ))
}

fn parse_wrapped_types(
    text: &str,
    prefix: &str,
    line: usize,
) -> Result<Vec<TypeId>, MirParseError> {
    let inner = text
        .strip_prefix(prefix)
        .and_then(|text| text.strip_suffix(')'))
        .ok_or_else(|| error(line, "nested signature types"))?;
    parse_semicolon_types(inner, line)
}

fn parse_captures(text: &str, line: usize) -> Result<Vec<MirCapture>, MirParseError> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|capture| {
            let parts: Vec<_> = capture.split(':').collect();
            if parts.len() != 4 {
                return Err(error(line, "capture schema"));
            }
            let (binding, slot) = parts[1]
                .split_once('@')
                .ok_or_else(|| error(line, "capture binding slot"))?;
            Ok(MirCapture {
                capture: CaptureId::from_raw(parse_named_prefix(parts[0], "cap", line)?),
                binding: BindingId::from_raw(parse_named_prefix(binding, "bind", line)?),
                slot: parse_u32(slot, line)?,
                type_id: TypeId::from_raw(parse_prefixed(parts[2], 't', line)?),
                mode: parse_capture_mode(parts[3], line)?,
            })
        })
        .collect()
}

fn parse_capture_mode(text: &str, line: usize) -> Result<MirCaptureMode, MirParseError> {
    match text {
        "value" => Ok(MirCaptureMode::Value),
        "cell" => Ok(MirCaptureMode::Cell),
        _ => Err(error(line, "capture mode")),
    }
}

fn parse_closure_captures(
    text: &str,
    line: usize,
) -> Result<Vec<MirClosureCapture>, MirParseError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|capture| {
            let (identity, value) = capture
                .split_once('=')
                .ok_or_else(|| error(line, "closure capture"))?;
            let (capture, binding_slot) = identity
                .split_once(':')
                .ok_or_else(|| error(line, "closure capture identity"))?;
            let (binding, slot) = binding_slot
                .split_once('@')
                .ok_or_else(|| error(line, "closure capture slot"))?;
            let parts: Vec<_> = value.split(':').collect();
            if parts.len() != 3 {
                return Err(error(line, "closure capture value"));
            }
            let self_reference = parts[0] == "self";
            let value = if self_reference {
                ValueId::from_raw(u32::MAX)
            } else {
                ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?)
            };
            Ok(MirClosureCapture {
                capture: CaptureId::from_raw(parse_named_prefix(capture, "cap", line)?),
                binding: BindingId::from_raw(parse_named_prefix(binding, "bind", line)?),
                slot: parse_u32(slot, line)?,
                value,
                self_reference,
                type_id: TypeId::from_raw(parse_prefixed(parts[1], 't', line)?),
                mode: parse_capture_mode(parts[2], line)?,
            })
        })
        .collect()
}

fn parse_block(lines: &[&str], start: usize) -> Result<(MirBlock, usize), MirParseError> {
    let number = start + 1;
    let header = lines[start]
        .trim()
        .strip_prefix('b')
        .ok_or_else(|| error(number, "expected block"))?;
    let (block, rest) = split_number(header, number)?;
    let rest = rest
        .strip_prefix('(')
        .ok_or_else(|| error(number, "malformed block arguments"))?;
    let (arguments, suffix) = rest
        .split_once(')')
        .ok_or_else(|| error(number, "malformed block arguments"))?;
    let cleanup = parse_cleanup_suffix(suffix, number)?;
    let arguments = parse_block_arguments(arguments, number)?;
    let mut instructions = Vec::new();
    let mut position = start + 1;
    while position < lines.len()
        && (lines[position].starts_with("    v") || lines[position].starts_with("    do v"))
    {
        instructions.push(parse_instruction(lines[position].trim(), position + 1)?);
        position += 1;
    }
    let terminator_line = lines
        .get(position)
        .filter(|line| line.starts_with("    "))
        .ok_or_else(|| error(position + 1, "missing terminator"))?;
    let terminator = parse_terminator(terminator_line.trim(), position + 1)?;
    Ok((
        MirBlock {
            block: BlockId::from_raw(block),
            cleanup,
            arguments,
            instructions,
            terminator,
        },
        position + 1,
    ))
}

fn parse_cleanup_suffix(
    suffix: &str,
    line: usize,
) -> Result<Option<MirCleanupBlock>, MirParseError> {
    if suffix == ":" {
        return Ok(None);
    }
    let cleanup = suffix
        .strip_prefix(" cleanup scope#")
        .and_then(|value| value.strip_suffix(':'))
        .ok_or_else(|| error(line, "malformed cleanup block"))?;
    let (scope, reason) = cleanup
        .split_once(" reason ")
        .ok_or_else(|| error(line, "malformed cleanup block"))?;
    let reason = match reason {
        "normal" => MirCleanupExitReason::Normal,
        "return" => MirCleanupExitReason::Return,
        "resultFailure" => MirCleanupExitReason::ResultFailure,
        "break" => MirCleanupExitReason::Break,
        "continue" => MirCleanupExitReason::Continue,
        "unwind" => MirCleanupExitReason::Unwind,
        "cancellation" => MirCleanupExitReason::Cancellation,
        _ => return Err(error(line, "cleanup exit reason")),
    };
    Ok(Some(MirCleanupBlock {
        scope: CleanupScopeId::from_raw(parse_u32(scope, line)?),
        reason,
    }))
}

fn parse_block_arguments(text: &str, line: usize) -> Result<Vec<MirBlockArgument>, MirParseError> {
    comma_parts(text)
        .map(|part| {
            let (value, type_id) = part
                .split_once(':')
                .ok_or_else(|| error(line, "malformed block argument"))?;
            Ok(MirBlockArgument {
                value: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
                type_id: TypeId::from_raw(parse_prefixed(type_id, 't', line)?),
                span: empty_span(),
            })
        })
        .collect()
}

fn parse_instruction(text: &str, line: usize) -> Result<MirInstruction, MirParseError> {
    if let Some(effect) = text.strip_prefix("do ") {
        let (instruction, operation) = effect
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed effect instruction"))?;
        let (operation, unwind) = parse_instruction_unwind(operation, line)?;
        let kind = parse_operation(operation, line)?;
        if !matches!(
            kind,
            MirInstructionKind::CallStandard { .. }
                | MirInstructionKind::CallDirect { .. }
                | MirInstructionKind::CallReferenced { .. }
                | MirInstructionKind::CallDirectMethod { .. }
                | MirInstructionKind::CallInterface { .. }
                | MirInstructionKind::CallBuiltinInterface { .. }
                | MirInstructionKind::CallIndirect { .. }
                | MirInstructionKind::GcSafePoint { .. }
                | MirInstructionKind::RetainRoot { .. }
                | MirInstructionKind::ReleaseRoot { .. }
                | MirInstructionKind::Pin { .. }
                | MirInstructionKind::Unpin { .. }
                | MirInstructionKind::WriteBarrier { .. }
                | MirInstructionKind::CaptureCellStore { .. }
                | MirInstructionKind::CaptureStore { .. }
        ) {
            return Err(error(line, "instruction does not have effect form"));
        }
        let effects = local_instruction_effects(&kind);
        return Ok(MirInstruction {
            result: ValueId::from_raw(parse_prefixed(instruction, 'v', line)?),
            result_type: None,
            kind,
            effects,
            effects_explicit: true,
            unwind,
            span: empty_span(),
        });
    }
    let (result, operation) = text
        .split_once(" = ")
        .ok_or_else(|| error(line, "malformed instruction"))?;
    let (value, type_id) = result
        .split_once(':')
        .ok_or_else(|| error(line, "malformed result"))?;
    let (operation, unwind) = parse_instruction_unwind(operation, line)?;
    let kind = parse_operation(operation, line)?;
    let effects = local_instruction_effects(&kind);
    Ok(MirInstruction {
        result: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        result_type: Some(TypeId::from_raw(parse_prefixed(type_id, 't', line)?)),
        kind,
        effects,
        effects_explicit: true,
        unwind,
        span: empty_span(),
    })
}

fn parse_instruction_unwind(
    operation: &str,
    line: usize,
) -> Result<(&str, MirUnwindAction), MirParseError> {
    let has_embedded_unwind = [
        "callDirect ",
        "callReference ",
        "callMethod ",
        "callInterface ",
        "callIndirect ",
    ]
    .iter()
    .any(|prefix| operation.starts_with(prefix));
    if has_embedded_unwind {
        return Ok((operation, MirUnwindAction::Propagate));
    }
    let Some((operation, target)) = operation.rsplit_once(" unwind cleanup:b") else {
        return Ok((operation, MirUnwindAction::Propagate));
    };
    Ok((
        operation,
        MirUnwindAction::Cleanup(BlockId::from_raw(parse_u32(target, line)?)),
    ))
}

#[allow(clippy::too_many_lines)]
fn parse_operation(text: &str, line: usize) -> Result<MirInstructionKind, MirParseError> {
    if let Some(constant) = parse_constant_operation(text, line)? {
        return Ok(constant);
    }
    if let Some(function) = text.strip_prefix("functionReference ") {
        return Ok(MirInstructionKind::FunctionReference(SymbolId::from_raw(
            parse_prefixed(function, 's', line)?,
        )));
    }
    if let Some(rest) = text.strip_prefix("task.create ") {
        let (dispatch, rest) = rest
            .split_once(" completion:t")
            .ok_or_else(|| error(line, "task creation dispatch"))?;
        let (completion_type, rest) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "task creation completion type"))?;
        let (object_map, arguments) = rest
            .split_once(" args ")
            .ok_or_else(|| error(line, "task creation arguments"))?;
        let dispatch = if let Some(function) = dispatch.strip_prefix("direct:s") {
            MirTaskDispatch::Direct(SymbolId::from_raw(parse_u32(function, line)?))
        } else if let Some(reference) = dispatch.strip_prefix("reference:b") {
            let (bubble, symbol) = reference
                .split_once(":s")
                .ok_or_else(|| error(line, "task reference identity"))?;
            MirTaskDispatch::Referenced(SymbolIdentity::new(
                BubbleId::from_raw(parse_u32(bubble, line)?),
                SymbolId::from_raw(parse_u32(symbol, line)?),
            ))
        } else if let Some(callee) = dispatch.strip_prefix("indirect:v") {
            MirTaskDispatch::Indirect(ValueId::from_raw(parse_u32(callee, line)?))
        } else {
            return Err(error(line, "task creation dispatch"));
        };
        return Ok(MirInstructionKind::TaskCreate {
            dispatch,
            arguments: parse_values(arguments, line)?,
            completion_type: TypeId::from_raw(parse_u32(completion_type, line)?),
            object_map: parse_object_map(object_map, line)?,
        });
    }
    if text == "cancelSourceCreate" {
        return Ok(MirInstructionKind::CancelSourceCreate);
    }
    if let Some(source) = text.strip_prefix("cancelSourceToken ") {
        return Ok(MirInstructionKind::CancelSourceToken {
            source: ValueId::from_raw(parse_prefixed(source, 'v', line)?),
        });
    }
    if let Some(source) = text.strip_prefix("cancelRequest ") {
        return Ok(MirInstructionKind::CancelRequest {
            source: ValueId::from_raw(parse_prefixed(source, 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("taskGroupCreate completion:t") {
        let (completion_type, rest) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "task group completion type"))?;
        let (object_map, operands) = rest
            .split_once(" cancel:v")
            .ok_or_else(|| error(line, "task group object map"))?;
        let (cancel, body) = operands
            .split_once(" body:v")
            .ok_or_else(|| error(line, "task group operands"))?;
        return Ok(MirInstructionKind::TaskGroupCreate {
            cancel: ValueId::from_raw(parse_u32(cancel, line)?),
            body: ValueId::from_raw(parse_u32(body, line)?),
            completion_type: TypeId::from_raw(parse_u32(completion_type, line)?),
            object_map: parse_object_map(object_map, line)?,
        });
    }
    if let Some(operands) = text.strip_prefix("taskStart ") {
        let (group, task) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::TaskStart { group, task });
    }
    if let Some(operands) = text.strip_prefix("string.concat ") {
        let (left, right) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::StringConcat { left, right });
    }
    if let Some(rest) = text.strip_prefix("string.format ") {
        let (kind, value) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "string format operation"))?;
        return Ok(MirInstructionKind::StringFormat {
            kind: parse_string_format_kind(kind, line)?,
            value: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        });
    }
    if let Some(values) = text.strip_prefix("tupleMake ") {
        return Ok(MirInstructionKind::TupleMake(parse_values(values, line)?));
    }
    if let Some(rest) = text.strip_prefix("tupleGet ") {
        let (index, tuple) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "tuple projection"))?;
        return Ok(MirInstructionKind::TupleGet {
            tuple: ValueId::from_raw(parse_prefixed(tuple, 'v', line)?),
            index: parse_u32(index, line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("arrayMake ") {
        let (element_map, values) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "array allocation map"))?;
        return Ok(MirInstructionKind::ArrayMake {
            elements: parse_values(values, line)?,
            element_map: parse_array_element_map(element_map, line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("arrayCreate ") {
        let (element_map, operands) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "array creation element map"))?;
        let (length, initial_value) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            element_map: parse_array_element_map(element_map, line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("tableMake ") {
        let (key_map, rest) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "table key allocation map"))?;
        let (value_map, entries) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "table value allocation map"))?;
        return Ok(MirInstructionKind::TableMake {
            entries: parse_table_entries(entries, line)?,
            key_map: parse_array_element_map(key_map, line)?,
            value_map: parse_array_element_map(value_map, line)?,
        });
    }
    if let Some(operands) = text.strip_prefix("tableGet ") {
        let (table, key) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::TableGet { table, key });
    }
    if let Some(operands) = text.strip_prefix("tableSet ") {
        let (key_map, rest) = operands
            .split_once(' ')
            .ok_or_else(|| error(line, "table set key map"))?;
        let (value_map, operands) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "table set value map"))?;
        let (table, key, value) = parse_three_values(operands, line)?;
        return Ok(MirInstructionKind::TableSet {
            table,
            key,
            value,
            key_map: parse_array_element_map(key_map, line)?,
            value_map: parse_array_element_map(value_map, line)?,
        });
    }
    if let Some(operands) = text.strip_prefix("arrayGet ") {
        let (array, index) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ArrayGet { array, index });
    }
    if let Some(array) = text.strip_prefix("arrayLength ") {
        return Ok(MirInstructionKind::ArrayLength {
            array: ValueId::from_raw(parse_prefixed(array, 'v', line)?),
        });
    }
    if let Some(operands) = text.strip_prefix("arrayGetChecked ") {
        let (array, index) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ArrayGetChecked { array, index });
    }
    if let Some(operands) = text.strip_prefix("arraySet ") {
        let (element_map, operands) = operands
            .split_once(' ')
            .ok_or_else(|| error(line, "array set element map"))?;
        let (array, index, value) = parse_three_values(operands, line)?;
        return Ok(MirInstructionKind::ArraySet {
            array,
            index,
            value,
            element_map: parse_array_element_map(element_map, line)?,
        });
    }
    if let Some(operands) = text.strip_prefix("arrayFill ") {
        let (element_map, operands) = operands
            .split_once(' ')
            .ok_or_else(|| error(line, "array fill element map"))?;
        let (array, value) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ArrayFill {
            array,
            value,
            element_map: parse_array_element_map(element_map, line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("listCreate ") {
        let (element_map, capacity) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "list creation element map"))?;
        let capacity = if capacity == "none" {
            None
        } else {
            Some(ValueId::from_raw(parse_prefixed(capacity, 'v', line)?))
        };
        return Ok(MirInstructionKind::ListCreate {
            capacity,
            element_map: parse_array_element_map(element_map, line)?,
        });
    }
    if let Some(list) = text.strip_prefix("listLength ") {
        return Ok(MirInstructionKind::ListLength {
            list: ValueId::from_raw(parse_prefixed(list, 'v', line)?),
        });
    }
    if let Some(operands) = text.strip_prefix("listGet ") {
        let (list, index) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ListGet { list, index });
    }
    if let Some(operands) = text.strip_prefix("listGetChecked ") {
        let (list, index) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ListGetChecked { list, index });
    }
    if let Some(operands) = text.strip_prefix("listSet ") {
        let (element_map, operands) = operands
            .split_once(' ')
            .ok_or_else(|| error(line, "list set element map"))?;
        let (list, index, value) = parse_three_values(operands, line)?;
        return Ok(MirInstructionKind::ListSet {
            list,
            index,
            value,
            element_map: parse_array_element_map(element_map, line)?,
        });
    }
    if let Some(operands) = text.strip_prefix("listAdd ") {
        let (element_map, operands) = operands
            .split_once(' ')
            .ok_or_else(|| error(line, "list append element map"))?;
        let (list, value) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ListAdd {
            list,
            value,
            element_map: parse_array_element_map(element_map, line)?,
        });
    }
    if let Some(operands) = text.strip_prefix("rangeCreate ") {
        let (first, last, step) = parse_three_values(operands, line)?;
        return Ok(MirInstructionKind::RangeCreate { first, last, step });
    }
    if let Some(optional) = text.strip_prefix("optionalIsPresent ") {
        return Ok(MirInstructionKind::OptionalIsPresent {
            optional: ValueId::from_raw(parse_prefixed(optional, 'v', line)?),
        });
    }
    if let Some(optional) = text.strip_prefix("optionalGet ") {
        return Ok(MirInstructionKind::OptionalGet {
            optional: ValueId::from_raw(parse_prefixed(optional, 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("resultMake ") {
        let parts: Vec<_> = rest.splitn(3, ' ').collect();
        if parts.len() != 3 {
            return Err(error(line, "malformed Result construction"));
        }
        return Ok(MirInstructionKind::ResultMake {
            result: parse_builtin_type_id(parts[0], line)?,
            case: ResultCaseId::from_raw(parse_hash(parts[1], "resultCase#", line)?),
            arguments: parse_values(parts[2], line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("iterationMake ") {
        let parts: Vec<_> = rest.splitn(3, ' ').collect();
        if parts.len() != 3 {
            return Err(error(line, "malformed Iteration construction"));
        }
        return Ok(MirInstructionKind::IterationMake {
            iteration: parse_builtin_type_id(parts[0], line)?,
            case: IterationCaseId::from_raw(parse_hash(parts[1], "iterationCase#", line)?),
            arguments: parse_values(parts[2], line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("errorMake ") {
        let parts: Vec<_> = rest.splitn(3, ' ').collect();
        if parts.len() != 3 {
            return Err(error(line, "malformed error construction"));
        }
        return Ok(MirInstructionKind::ErrorMake {
            error: ErrorId::from_raw(parse_prefixed(parts[0], 'e', line)?),
            case: ErrorCaseId::from_raw(parse_hash(parts[1], "errorCase#", line)?),
            arguments: parse_values(parts[2], line)?,
        });
    }
    if let Some(value) = text.strip_prefix("resultIsOk ") {
        let (definition, value) = value
            .split_once(' ')
            .ok_or_else(|| error(line, "Result test"))?;
        return Ok(MirInstructionKind::ResultIsOk {
            result: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
            definition: parse_builtin_type_id(definition, line)?,
        });
    }
    if let Some(value) = text.strip_prefix("resultGetOk ") {
        let (definition, value) = value
            .split_once(' ')
            .ok_or_else(|| error(line, "Result projection"))?;
        return Ok(MirInstructionKind::ResultGetOk {
            result: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
            definition: parse_builtin_type_id(definition, line)?,
        });
    }
    if let Some(value) = text.strip_prefix("resultGetError ") {
        let (definition, value) = value
            .split_once(' ')
            .ok_or_else(|| error(line, "Result projection"))?;
        return Ok(MirInstructionKind::ResultGetError {
            result: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
            definition: parse_builtin_type_id(definition, line)?,
        });
    }
    if let Some(operation) = parse_numeric_operation(text, line)? {
        return Ok(operation);
    }
    for (name, constructor) in binary_operations() {
        if let Some(operands) = text
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            let (left, right) = parse_two_values(operands, line)?;
            return Ok(constructor(left, right));
        }
    }
    for (name, constructor) in unary_operations() {
        if let Some(operand) = text
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            return Ok(constructor(ValueId::from_raw(parse_prefixed(
                operand, 'v', line,
            )?)));
        }
    }
    if text.starts_with("callStandard")
        || text.starts_with("callDirect")
        || text.starts_with("callReference")
        || text.starts_with("callIndirect")
        || text.starts_with("call.interface")
        || text.starts_with("call.builtinInterface")
    {
        return parse_call_operation(text, line);
    }
    if let Some(rest) = text.strip_prefix("iteration.isItem definition#") {
        let mut parts = rest.split_whitespace();
        let definition = required(&mut parts, line)?;
        let item_case = required(&mut parts, line)?;
        let end_case = required(&mut parts, line)?;
        let iteration = required(&mut parts, line)?;
        return Ok(MirInstructionKind::IterationIsItem {
            iteration: ValueId::from_raw(parse_prefixed(iteration, 'v', line)?),
            definition: BuiltinTypeId::from_raw(parse_u32(definition, line)?),
            item_case: IterationCaseId::from_raw(parse_hash(item_case, "case#", line)?),
            end_case: IterationCaseId::from_raw(parse_hash(end_case, "endCase#", line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("iteration.getItem definition#") {
        let mut parts = rest.split_whitespace();
        let definition = required(&mut parts, line)?;
        let item_case = required(&mut parts, line)?;
        let iteration = required(&mut parts, line)?;
        return Ok(MirInstructionKind::IterationGetItem {
            iteration: ValueId::from_raw(parse_prefixed(iteration, 'v', line)?),
            definition: BuiltinTypeId::from_raw(parse_u32(definition, line)?),
            item_case: IterationCaseId::from_raw(parse_hash(item_case, "case#", line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("interface.upcast ") {
        let mut parts = rest.split_whitespace();
        let value = ValueId::from_raw(parse_prefixed(required(&mut parts, line)?, 'v', line)?);
        let interface = required(&mut parts, line)?;
        let interface = match interface.chars().next() {
            Some('i') => NominalInterfaceId::User(InterfaceId::from_raw(parse_prefixed(
                interface, 'i', line,
            )?)),
            Some('b') => NominalInterfaceId::Builtin(BuiltinTypeId::from_raw(parse_prefixed(
                interface, 'b', line,
            )?)),
            _ => return Err(error(line, "interface identity")),
        };
        return Ok(MirInstructionKind::InterfaceUpcast { value, interface });
    }
    if let Some(rest) = text.strip_prefix("gcSafePoint ") {
        let (safe_point, roots) = rest
            .split_once(" roots ")
            .ok_or_else(|| error(line, "safe point roots"))?;
        let safe_point = SafePointId::new(
            parse_prefixed(safe_point, 'p', line)
                .or_else(|_| parse_hash(safe_point, "sp", line))?,
        );
        let roots = parse_values(roots, line)?;
        let root_slots = (0..roots.len())
            .map(|index| RootSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
            .collect();
        return Ok(MirInstructionKind::GcSafePoint {
            safe_point,
            roots,
            stack_map: StackMap::new(safe_point, root_slots)
                .map_err(|_| error(line, "stack map"))?,
        });
    }
    if let Some(value) = text.strip_prefix("retainRoot ") {
        return Ok(MirInstructionKind::RetainRoot {
            value: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        });
    }
    if let Some(value) = text.strip_prefix("releaseRoot ") {
        return Ok(MirInstructionKind::ReleaseRoot {
            handle: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        });
    }
    if let Some(value) = text.strip_prefix("pin ") {
        return Ok(MirInstructionKind::Pin {
            value: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        });
    }
    if let Some(value) = text.strip_prefix("unpin ") {
        return Ok(MirInstructionKind::Unpin {
            handle: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("writeBarrier ") {
        return parse_write_barrier(rest, line);
    }
    if let Some(rest) = text.strip_prefix("captureCell.allocate ") {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 4 {
            return Err(error(line, "capture cell allocation"));
        }
        return Ok(MirInstructionKind::CaptureCellAllocate {
            binding: BindingId::from_raw(parse_named_prefix(parts[0], "bind", line)?),
            initial: ValueId::from_raw(parse_prefixed(parts[1], 'v', line)?),
            value_type: TypeId::from_raw(parse_prefixed(parts[2], 't', line)?),
            object_map: parse_object_map(parts[3], line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("captureCell.load ") {
        return Ok(MirInstructionKind::CaptureCellLoad {
            cell: ValueId::from_raw(parse_prefixed(rest, 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("captureCell.store ") {
        let (cell, value) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "capture cell store"))?;
        return Ok(MirInstructionKind::CaptureCellStore {
            cell: ValueId::from_raw(parse_prefixed(cell, 'v', line)?),
            value: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("closureEnvironment.allocate ") {
        let (head, captures) = rest
            .split_once(" captures[")
            .and_then(|(head, captures)| {
                captures.strip_suffix(']').map(|captures| (head, captures))
            })
            .ok_or_else(|| error(line, "closure environment allocation"))?;
        let parts: Vec<_> = head.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(error(line, "closure environment header"));
        }
        return Ok(MirInstructionKind::ClosureEnvironmentAllocate {
            owner: SymbolId::from_raw(parse_prefixed(parts[0], 's', line)?),
            function: NestedFunctionId::from_raw(parse_named_prefix(parts[1], "nf", line)?),
            object_map: parse_object_map(parts[2], line)?,
            captures: parse_closure_captures(captures, line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("capture.load ") {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(error(line, "capture load"));
        }
        return Ok(MirInstructionKind::CaptureLoad {
            capture: CaptureId::from_raw(parse_named_prefix(parts[0], "cap", line)?),
            slot: parse_hash(parts[1], "slot#", line)?,
            mode: parse_capture_mode(parts[2], line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("capture.cell ") {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(error(line, "capture cell reference"));
        }
        return Ok(MirInstructionKind::CaptureCellReference {
            capture: CaptureId::from_raw(parse_named_prefix(parts[0], "cap", line)?),
            slot: parse_hash(parts[1], "slot#", line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("capture.store ") {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(error(line, "capture store"));
        }
        return Ok(MirInstructionKind::CaptureStore {
            capture: CaptureId::from_raw(parse_named_prefix(parts[0], "cap", line)?),
            slot: parse_hash(parts[1], "slot#", line)?,
            value: ValueId::from_raw(parse_prefixed(parts[2], 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("fieldGet ") {
        let mut parts = rest.split_whitespace();
        return Ok(MirInstructionKind::FieldGet {
            base: ValueId::from_raw(parse_prefixed(required(&mut parts, line)?, 'v', line)?),
            field: FieldId::from_raw(parse_hash(required(&mut parts, line)?, "field#", line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("fieldSet ") {
        let mut parts = rest.split_whitespace();
        return Ok(MirInstructionKind::FieldSet {
            base: ValueId::from_raw(parse_prefixed(required(&mut parts, line)?, 'v', line)?),
            field: FieldId::from_raw(parse_hash(required(&mut parts, line)?, "field#", line)?),
            value: ValueId::from_raw(parse_prefixed(required(&mut parts, line)?, 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("unionMake s") {
        let mut parts = rest.splitn(3, ' ');
        return Ok(MirInstructionKind::UnionMake {
            union: SymbolId::from_raw(parse_u32(required(&mut parts, line)?, line)?),
            case: UnionCaseId::from_raw(parse_hash(required(&mut parts, line)?, "case#", line)?),
            arguments: parse_values(required(&mut parts, line)?, line)?,
        });
    }
    if text.starts_with("recordMake s") || text.starts_with("recordUpdate s") {
        return parse_record_operation(text, line);
    }
    if let Some(rest) = text.strip_prefix("classMake c") {
        let (head, fields) = rest
            .split_once(" {")
            .ok_or_else(|| error(line, "class fields"))?;
        let (class, object_map) = head
            .split_once(' ')
            .ok_or_else(|| error(line, "class allocation map"))?;
        let fields = fields
            .strip_suffix('}')
            .ok_or_else(|| error(line, "class fields"))?;
        return Ok(MirInstructionKind::ClassMake {
            class: ClassId::from_raw(parse_u32(class, line)?),
            fields: parse_field_values(fields, line)?,
            object_map: parse_object_map(object_map, line)?,
        });
    }
    Err(error(line, "unknown instruction"))
}

fn parse_constant_operation(
    text: &str,
    line: usize,
) -> Result<Option<MirInstructionKind>, MirParseError> {
    if let Some(value) = text.strip_prefix("const.integer ") {
        let (kind, value) = value
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed integer constant"))?;
        return IntegerValue::parse_decimal(value, parse_integer_kind(kind, line)?)
            .map(MirInstructionKind::IntegerConstant)
            .map(Some)
            .map_err(|_| error(line, "invalid integer"));
    }
    if let Some(value) = text.strip_prefix("const.float ") {
        let (kind, bits) = value
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed float constant"))?;
        let kind = parse_float_kind(kind, line)?;
        let bits = u64::from_str_radix(
            bits.strip_prefix("0x")
                .ok_or_else(|| error(line, "float bits"))?,
            16,
        )
        .map_err(|_| error(line, "float bits"))?;
        let value = match kind {
            FloatKind::Float32 => {
                FloatValue::Float32(u32::try_from(bits).map_err(|_| error(line, "Float32 bits"))?)
            }
            FloatKind::Float64 => FloatValue::Float64(bits),
        };
        return Ok(Some(MirInstructionKind::FloatConstant(value)));
    }
    if let Some(value) = text.strip_prefix("const.string ") {
        return parse_string(value, line)
            .map(MirInstructionKind::StringConstant)
            .map(Some);
    }
    if let Some(value) = text.strip_prefix("const.boolean ") {
        return match value {
            "true" => Ok(Some(MirInstructionKind::BooleanConstant(true))),
            "false" => Ok(Some(MirInstructionKind::BooleanConstant(false))),
            _ => Err(error(line, "invalid boolean")),
        };
    }
    if text == "const.nil" {
        return Ok(Some(MirInstructionKind::NilConstant));
    }
    if let Some(value) = text.strip_prefix("enum.case s") {
        let components: Vec<_> = value.split_whitespace().collect();
        if let [definition, case, discriminant] = components.as_slice() {
            return Ok(Some(MirInstructionKind::EnumConstant {
                definition: SymbolId::from_raw(parse_u32(definition, line)?),
                case: EnumCaseId::from_raw(
                    case.strip_prefix("ec")
                        .ok_or_else(|| error(line, "enum case"))
                        .and_then(|case| parse_u32(case, line))?,
                ),
                discriminant: parse_u32(discriminant, line)?,
            }));
        }
        return Err(error(line, "malformed enum constant"));
    }
    Ok(None)
}

fn parse_call_operation(text: &str, line: usize) -> Result<MirInstructionKind, MirParseError> {
    if let Some(rest) = text.strip_prefix("callStandard sf") {
        let (call, effects) = rest
            .split_once(" effects[")
            .ok_or_else(|| error(line, "standard call effect contract"))?;
        let effects = effects
            .strip_suffix(']')
            .ok_or_else(|| error(line, "standard call effect contract"))?;
        let (function, values) = call
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed standard call"))?;
        return Ok(MirInstructionKind::CallStandard {
            function: StandardFunctionId::from_raw(parse_u32(function, line)?),
            arguments: parse_values(values, line)?,
            declared_effects: parse_effects(effects, line)?,
        });
    }
    let (text, declared_effects, unwind) =
        if let Some((call, contract)) = text.split_once(" effects[") {
            let (effects, unwind) = contract
                .split_once("] unwind ")
                .ok_or_else(|| error(line, "call effect contract"))?;
            (
                call,
                parse_effects(effects, line)?,
                parse_unwind_action(unwind, line)?,
            )
        } else {
            (text, MirEffectSummary::empty(), MirUnwindAction::Propagate)
        };
    if let Some(rest) = text.strip_prefix("callDirect s") {
        let (function, values) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed direct call"))?;
        return Ok(MirInstructionKind::CallDirect {
            function: SymbolId::from_raw(parse_u32(function, line)?),
            arguments: parse_values(values, line)?,
            declared_effects,
            unwind,
        });
    }
    if let Some(rest) = text.strip_prefix("callReference b") {
        let (identity, values) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed referenced call"))?;
        let (bubble, symbol) = identity
            .split_once(":s")
            .ok_or_else(|| error(line, "referenced call identity"))?;
        return Ok(MirInstructionKind::CallReferenced {
            function: SymbolIdentity::new(
                BubbleId::from_raw(parse_u32(bubble, line)?),
                SymbolId::from_raw(parse_u32(symbol, line)?),
            ),
            arguments: parse_values(values, line)?,
            declared_effects,
            unwind,
        });
    }
    if let Some(rest) = text.strip_prefix("callIndirect v") {
        let (callee, values) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed indirect call"))?;
        return Ok(MirInstructionKind::CallIndirect {
            callee: ValueId::from_raw(parse_u32(callee, line)?),
            arguments: parse_values(values, line)?,
            declared_effects,
            unwind,
        });
    }
    if let Some(rest) = text.strip_prefix("call.interface i") {
        let (interface, rest) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed interface call"))?;
        let rest = rest
            .strip_prefix("im")
            .ok_or_else(|| error(line, "interface method"))?;
        let (method, rest) = rest
            .split_once(" slot#")
            .ok_or_else(|| error(line, "interface slot"))?;
        let (slot, values) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "interface arguments"))?;
        return Ok(MirInstructionKind::CallInterface {
            interface: InterfaceId::from_raw(parse_u32(interface, line)?),
            method: InterfaceMethodId::from_raw(parse_u32(method, line)?),
            slot: parse_u32(slot, line)?,
            arguments: parse_values(values, line)?,
            declared_effects,
            unwind,
        });
    }
    if let Some(rest) = text.strip_prefix("call.builtinInterface interface#") {
        let (interface, rest) = rest
            .split_once(" method#")
            .ok_or_else(|| error(line, "built-in interface identity"))?;
        let (method, values) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "built-in interface arguments"))?;
        return Ok(MirInstructionKind::CallBuiltinInterface {
            interface: BuiltinTypeId::from_raw(parse_u32(interface, line)?),
            method: IterationProtocolMethodId::from_raw(parse_u32(method, line)?),
            arguments: parse_values(values, line)?,
            declared_effects,
            unwind,
        });
    }
    let rest = text
        .strip_prefix("callDirectMethod m")
        .ok_or_else(|| error(line, "malformed direct method call"))?;
    let (method, values) = rest
        .split_once(' ')
        .ok_or_else(|| error(line, "malformed direct method call"))?;
    Ok(MirInstructionKind::CallDirectMethod {
        method: MethodId::from_raw(parse_u32(method, line)?),
        arguments: parse_values(values, line)?,
        declared_effects,
        unwind,
    })
}

fn parse_effects(text: &str, line: usize) -> Result<MirEffectSummary, MirParseError> {
    comma_parts(text)
        .map(|effect| match effect {
            "Allocates" => Ok(MirEffect::Allocates),
            "WritesManagedReference" => Ok(MirEffect::WritesManagedReference),
            "Synchronizes" => Ok(MirEffect::Synchronizes),
            "MayTrap" => Ok(MirEffect::MayTrap),
            "MayUnwind" => Ok(MirEffect::MayUnwind),
            "Suspends" => Ok(MirEffect::Suspends),
            "Blocks" => Ok(MirEffect::Blocks),
            "UnsafeMemory" => Ok(MirEffect::UnsafeMemory),
            "ForeignFunction" => Ok(MirEffect::ForeignFunction),
            "AmbientIo" => Ok(MirEffect::AmbientIo),
            "CompilerQuery" => Ok(MirEffect::CompilerQuery),
            "GcSafePoint" => Ok(MirEffect::GcSafePoint),
            "Roots" => Ok(MirEffect::Roots),
            _ => Err(error(line, "effect name")),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(MirEffectSummary::from_effects)
}

fn parse_unwind_action(text: &str, line: usize) -> Result<MirUnwindAction, MirParseError> {
    if text == "propagate" {
        return Ok(MirUnwindAction::Propagate);
    }
    text.strip_prefix("cleanup:b")
        .ok_or_else(|| error(line, "unwind action"))
        .and_then(|block| parse_u32(block, line))
        .map(BlockId::from_raw)
        .map(MirUnwindAction::Cleanup)
}

fn parse_array_element_map(text: &str, line: usize) -> Result<ArrayElementMap, MirParseError> {
    match text {
        "scalar" => Ok(ArrayElementMap::Scalar),
        "managed" => Ok(ArrayElementMap::ManagedReference),
        _ => Err(error(line, "array element map")),
    }
}

fn parse_object_map(text: &str, line: usize) -> Result<ObjectMap, MirParseError> {
    let inner = text
        .strip_prefix("map[")
        .and_then(|text| text.strip_suffix(']'))
        .ok_or_else(|| error(line, "object map"))?;
    let (slot_count, references) = inner
        .split_once(':')
        .ok_or_else(|| error(line, "object map"))?;
    let slot_count = parse_u32(slot_count, line)?;
    let references = comma_parts(references)
        .map(|slot| parse_u32(slot, line).map(ObjectSlot::new))
        .collect::<Result<_, _>>()?;
    ObjectMap::new(slot_count, references).map_err(|_| error(line, "object map"))
}

fn parse_write_barrier(rest: &str, line: usize) -> Result<MirInstructionKind, MirParseError> {
    let parts = rest.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 7 || parts[1] != "slot" || parts[3] != "previous" || parts[5] != "value" {
        return Err(error(line, "write barrier"));
    }
    Ok(MirInstructionKind::WriteBarrier {
        owner: ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?),
        slot: ObjectSlot::new(parse_u32(parts[2], line)?),
        previous: parse_optional_value(parts[4], line)?,
        value: parse_optional_value(parts[6], line)?,
    })
}

fn parse_optional_value(text: &str, line: usize) -> Result<Option<ValueId>, MirParseError> {
    if text == "nil" {
        Ok(None)
    } else {
        parse_prefixed(text, 'v', line)
            .map(ValueId::from_raw)
            .map(Some)
    }
}

fn parse_record_operation(text: &str, line: usize) -> Result<MirInstructionKind, MirParseError> {
    let update = text.starts_with("recordUpdate s");
    let prefix = if update {
        "recordUpdate s"
    } else {
        "recordMake s"
    };
    let rest = text
        .strip_prefix(prefix)
        .ok_or_else(|| error(line, "record operation"))?;
    let (head, fields) = rest
        .split_once(" {")
        .ok_or_else(|| error(line, "record fields"))?;
    let fields = fields
        .strip_suffix('}')
        .ok_or_else(|| error(line, "record fields"))?;
    let mut head = head.split_whitespace();
    let record = SymbolId::from_raw(parse_u32(required(&mut head, line)?, line)?);
    let base = head
        .next()
        .map(|value| parse_prefixed(value, 'v', line).map(ValueId::from_raw))
        .transpose()?;
    let fields = comma_parts(fields)
        .map(|field| {
            let (field, value) = field
                .split_once('=')
                .ok_or_else(|| error(line, "record field"))?;
            Ok((
                FieldId::from_raw(parse_hash(field, "field#", line)?),
                ValueId::from_raw(parse_prefixed(value, 'v', line)?),
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    match base {
        Some(base) if update => Ok(MirInstructionKind::RecordUpdate {
            record,
            base,
            fields,
        }),
        None if !update => Ok(MirInstructionKind::RecordMake { record, fields }),
        _ => Err(error(line, "record base mismatch")),
    }
}

fn parse_field_values(fields: &str, line: usize) -> Result<Vec<(FieldId, ValueId)>, MirParseError> {
    comma_parts(fields)
        .map(|field| {
            let (field, value) = field
                .split_once('=')
                .ok_or_else(|| error(line, "field value"))?;
            Ok((
                FieldId::from_raw(parse_hash(field, "field#", line)?),
                ValueId::from_raw(parse_prefixed(value, 'v', line)?),
            ))
        })
        .collect()
}

fn parse_terminator(text: &str, line: usize) -> Result<MirTerminator, MirParseError> {
    if text == "missing" {
        return Ok(MirTerminator::Missing);
    }
    if text == "unreachable" {
        return Ok(MirTerminator::Unreachable);
    }
    if let Some(kind) = text.strip_prefix("trap ") {
        return Ok(MirTerminator::Trap(Trap::new(parse_trap_kind(kind, line)?)));
    }
    if let Some(kind) = text.strip_prefix("panic ") {
        return Ok(MirTerminator::Panic(parse_panic_payload(kind, line)?));
    }
    if let Some(reason) = text.strip_prefix("resumeUnwind ") {
        return Ok(MirTerminator::ContinueUnwind(parse_unwind_reason(
            reason, line,
        )?));
    }
    if text == "resumeCurrentUnwind" {
        return Ok(MirTerminator::ResumeUnwind);
    }
    if let Some(rest) = text.strip_prefix("suspend.task ") {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 10 {
            return Err(error(line, "malformed task suspension"));
        }
        let task = ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?);
        let result_type = TypeId::from_raw(parse_named_prefix(parts[1], "result:t", line)?);
        let resume = BlockId::from_raw(parse_named_prefix(parts[2], "resume:b", line)?);
        let cancellation = BlockId::from_raw(parse_named_prefix(parts[3], "cancellation:b", line)?);
        let cancellation_mode = match parts[4]
            .strip_prefix("cancellation-mode:")
            .ok_or_else(|| error(line, "task suspension cancellation mode"))?
        {
            "observe" => MirCancellationMode::Observe,
            "masked" => MirCancellationMode::Masked,
            _ => return Err(error(line, "task suspension cancellation mode")),
        };
        let unwind = parts[5]
            .strip_prefix("unwind:")
            .ok_or_else(|| error(line, "task suspension unwind"))?;
        let unwind = if unwind == "propagate" {
            MirUnwindAction::Propagate
        } else if let Some(target) = unwind.strip_prefix("cleanup:b") {
            MirUnwindAction::Cleanup(BlockId::from_raw(parse_u32(target, line)?))
        } else {
            return Err(error(line, "task suspension unwind"));
        };
        let safe_point = SafePointId::new(parse_named_prefix(parts[6], "safePoint:sp", line)?);
        let state = CoroutineStateId::from_raw(parse_named_prefix(parts[7], "state:cs", line)?);
        let frame = parts[8]
            .strip_prefix("frame[")
            .and_then(|frame| frame.strip_suffix(']'))
            .ok_or_else(|| error(line, "task suspension frame"))?;
        let slots = if frame.is_empty() {
            Vec::new()
        } else {
            frame
                .split(',')
                .map(|slot| {
                    let (value, type_id) = slot
                        .split_once(':')
                        .ok_or_else(|| error(line, "task suspension frame slot"))?;
                    Ok(MirFrameSlot {
                        value: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
                        type_id: TypeId::from_raw(parse_prefixed(type_id, 't', line)?),
                    })
                })
                .collect::<Result<Vec<_>, MirParseError>>()?
        };
        let roots = parts[9]
            .strip_prefix("roots[")
            .and_then(|roots| roots.strip_suffix(']'))
            .ok_or_else(|| error(line, "task suspension roots"))?;
        let roots = if roots.is_empty() {
            Vec::new()
        } else {
            roots
                .split(',')
                .map(|root| parse_u32(root, line).map(RootSlot::new))
                .collect::<Result<Vec<_>, _>>()?
        };
        return Ok(MirTerminator::Suspend {
            operation: MirSuspendOperation::Task { task, result_type },
            resume,
            cancellation,
            cancellation_mode,
            unwind,
            safe_point,
            live_frame: MirLiveFrame {
                state,
                slots,
                stack_map: StackMap::new(safe_point, roots)
                    .map_err(|_| error(line, "task suspension stack map"))?,
            },
        });
    }
    if let Some(values) = text.strip_prefix("return ") {
        return Ok(MirTerminator::Return {
            values: parse_values(values, line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("branch ") {
        let (target, values) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "malformed branch"))?;
        return Ok(MirTerminator::Branch {
            target: BlockId::from_raw(parse_prefixed(target, 'b', line)?),
            arguments: parse_values(values, line)?,
        });
    }
    if let Some(rest) = text.strip_prefix("condBranch ") {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(error(line, "malformed conditional branch"));
        }
        return Ok(MirTerminator::ConditionalBranch {
            condition: ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?),
            when_true: BlockId::from_raw(parse_prefixed(parts[1], 'b', line)?),
            when_false: BlockId::from_raw(parse_prefixed(parts[2], 'b', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("union.switch ") {
        let (head, arms_text) = rest
            .split_once(" [")
            .and_then(|(head, arms)| arms.strip_suffix(']').map(|arms| (head, arms)))
            .ok_or_else(|| error(line, "malformed union switch"))?;
        let parts: Vec<_> = head.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(error(line, "malformed union switch header"));
        }
        let arms = comma_parts(arms_text)
            .map(|arm| {
                let (case, target) = arm
                    .split_once(':')
                    .ok_or_else(|| error(line, "malformed union switch arm"))?;
                Ok(MirUnionSwitchArm {
                    case: UnionCaseId::from_raw(parse_hash(case, "case#", line)?),
                    target: BlockId::from_raw(parse_prefixed(target, 'b', line)?),
                })
            })
            .collect::<Result<Vec<_>, MirParseError>>()?;
        return Ok(MirTerminator::UnionSwitch {
            scrutinee: ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?),
            union: SymbolId::from_raw(parse_prefixed(parts[1], 's', line)?),
            arms,
        });
    }
    if let Some(rest) = text.strip_prefix("errorSwitch ") {
        let (head, arms_text) = rest
            .split_once(" [")
            .and_then(|(head, arms)| arms.strip_suffix(']').map(|arms| (head, arms)))
            .ok_or_else(|| error(line, "malformed error switch"))?;
        let parts: Vec<_> = head.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(error(line, "malformed error switch header"));
        }
        let arms = comma_parts(arms_text)
            .map(|arm| {
                let (case, target) = arm
                    .split_once(':')
                    .ok_or_else(|| error(line, "malformed error switch arm"))?;
                Ok(MirErrorSwitchArm {
                    case: ErrorCaseId::from_raw(parse_hash(case, "errorCase#", line)?),
                    target: BlockId::from_raw(parse_prefixed(target, 'b', line)?),
                })
            })
            .collect::<Result<Vec<_>, MirParseError>>()?;
        return Ok(MirTerminator::ErrorSwitch {
            scrutinee: ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?),
            error: ErrorId::from_raw(parse_prefixed(parts[1], 'e', line)?),
            arms,
        });
    }
    Err(error(line, "unknown terminator"))
}

fn parse_trap_kind(text: &str, line: usize) -> Result<TrapKind, MirParseError> {
    match text {
        "IntegerOverflow" => Ok(TrapKind::IntegerOverflow),
        "DivisionByZero" => Ok(TrapKind::DivisionByZero),
        "NumericConversion" => Ok(TrapKind::NumericConversion),
        "InvalidRangeStep" => Ok(TrapKind::InvalidRangeStep),
        "BoundsViolation" => Ok(TrapKind::BoundsViolation),
        "ConcurrentModification" => Ok(TrapKind::ConcurrentModification),
        "ImpossibleState" => Ok(TrapKind::ImpossibleState),
        _ => Err(error(line, "trap kind")),
    }
}

fn parse_panic_payload(text: &str, line: usize) -> Result<PanicPayload, MirParseError> {
    if text == "RuntimeInvariant" {
        return Ok(PanicPayload::new(PanicKind::RuntimeInvariant));
    }
    if text == "DoublePanic" {
        return Ok(PanicPayload::new(PanicKind::DoublePanic));
    }
    let Some(values) = text
        .strip_prefix("OutOfMemory(")
        .and_then(|text| text.strip_suffix(')'))
    else {
        return Err(error(line, "panic payload"));
    };
    let (objects, slots) = values
        .split_once(',')
        .ok_or_else(|| error(line, "out-of-memory payload"))?;
    Ok(PanicPayload::out_of_memory(
        objects
            .parse()
            .map_err(|_| error(line, "out-of-memory object count"))?,
        slots
            .parse()
            .map_err(|_| error(line, "out-of-memory slot count"))?,
    ))
}

fn parse_unwind_reason(text: &str, line: usize) -> Result<UnwindReason, MirParseError> {
    if text == "Cancellation" {
        Ok(UnwindReason::Cancellation)
    } else {
        parse_panic_payload(text, line).map(UnwindReason::Panic)
    }
}

type BinaryConstructor = fn(ValueId, ValueId) -> MirInstructionKind;
type UnaryConstructor = fn(ValueId) -> MirInstructionKind;

fn binary_operations() -> [(&'static str, BinaryConstructor); 4] {
    [
        ("booleanAnd", |left, right| MirInstructionKind::BooleanAnd {
            left,
            right,
        }),
        ("booleanOr", |left, right| MirInstructionKind::BooleanOr {
            left,
            right,
        }),
        ("compareEqual", |left, right| {
            MirInstructionKind::CompareEqual { left, right }
        }),
        ("compareNotEqual", |left, right| {
            MirInstructionKind::CompareNotEqual { left, right }
        }),
    ]
}

fn unary_operations() -> [(&'static str, UnaryConstructor); 1] {
    [("booleanNot", |operand| MirInstructionKind::BooleanNot {
        operand,
    })]
}

fn parse_numeric_operation(
    text: &str,
    line: usize,
) -> Result<Option<MirInstructionKind>, MirParseError> {
    if !(text.starts_with("integer.") || text.starts_with("float.") || text.starts_with("numeric."))
    {
        return Ok(None);
    }
    let parts = text.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 4 && parts[0].starts_with("numeric.") {
        let operand = ValueId::from_raw(parse_prefixed(parts[3], 'v', line)?);
        return Ok(Some(match parts[0] {
            "numeric.integerToInteger" => MirInstructionKind::ConvertInteger {
                source: parse_integer_kind(parts[1], line)?,
                target: parse_integer_kind(parts[2], line)?,
                operand,
            },
            "numeric.integerToFloat" => MirInstructionKind::ConvertIntegerToFloat {
                source: parse_integer_kind(parts[1], line)?,
                target: parse_float_kind(parts[2], line)?,
                operand,
            },
            "numeric.floatToInteger" => MirInstructionKind::ConvertFloatToInteger {
                source: parse_float_kind(parts[1], line)?,
                target: parse_integer_kind(parts[2], line)?,
                operand,
            },
            "numeric.floatToFloat" => MirInstructionKind::ConvertFloat {
                source: parse_float_kind(parts[1], line)?,
                target: parse_float_kind(parts[2], line)?,
                operand,
            },
            _ => return Err(error(line, "unknown numeric conversion")),
        }));
    }
    if parts.len() == 3 && matches!(parts[0], "integer.negate" | "float.negate") {
        let operand = ValueId::from_raw(parse_prefixed(parts[2], 'v', line)?);
        return Ok(Some(match parts[0] {
            "integer.negate" => MirInstructionKind::IntegerNegate {
                kind: parse_integer_kind(parts[1], line)?,
                operand,
            },
            "float.negate" => MirInstructionKind::FloatNegate {
                kind: parse_float_kind(parts[1], line)?,
                operand,
            },
            _ => unreachable!(),
        }));
    }
    if parts.len() != 4 {
        return Err(error(line, "malformed numeric operation"));
    }
    let left = ValueId::from_raw(parse_prefixed(parts[2], 'v', line)?);
    let right = ValueId::from_raw(parse_prefixed(parts[3], 'v', line)?);
    let operation = match parts[0] {
        "integer.checkedAdd" => MirInstructionKind::CheckedIntegerAdd {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.checkedSubtract" => MirInstructionKind::CheckedIntegerSubtract {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.checkedMultiply" => MirInstructionKind::CheckedIntegerMultiply {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.checkedDivide" => MirInstructionKind::CheckedIntegerDivide {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.checkedRemainder" => MirInstructionKind::CheckedIntegerRemainder {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.compareLess" => MirInstructionKind::CompareIntegerLess {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.compareLessOrEqual" => MirInstructionKind::CompareIntegerLessOrEqual {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.compareGreater" => MirInstructionKind::CompareIntegerGreater {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "integer.compareGreaterOrEqual" => MirInstructionKind::CompareIntegerGreaterOrEqual {
            kind: parse_integer_kind(parts[1], line)?,
            left,
            right,
        },
        "float.add" => MirInstructionKind::FloatAdd {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        "float.subtract" => MirInstructionKind::FloatSubtract {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        "float.multiply" => MirInstructionKind::FloatMultiply {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        "float.divide" => MirInstructionKind::FloatDivide {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        "float.compareLess" => MirInstructionKind::CompareFloatLess {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        "float.compareLessOrEqual" => MirInstructionKind::CompareFloatLessOrEqual {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        "float.compareGreater" => MirInstructionKind::CompareFloatGreater {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        "float.compareGreaterOrEqual" => MirInstructionKind::CompareFloatGreaterOrEqual {
            kind: parse_float_kind(parts[1], line)?,
            left,
            right,
        },
        _ => return Err(error(line, "unknown numeric operation")),
    };
    Ok(Some(operation))
}

fn parse_integer_kind(text: &str, line: usize) -> Result<IntegerKind, MirParseError> {
    match text {
        "Int8" => Ok(IntegerKind::Int8),
        "Int16" => Ok(IntegerKind::Int16),
        "Int32" => Ok(IntegerKind::Int32),
        "Int64" => Ok(IntegerKind::Int64),
        "UInt8" => Ok(IntegerKind::UInt8),
        "UInt16" => Ok(IntegerKind::UInt16),
        "UInt32" => Ok(IntegerKind::UInt32),
        "UInt64" => Ok(IntegerKind::UInt64),
        _ => Err(error(line, "integer kind")),
    }
}

fn parse_string_format_kind(
    text: &str,
    line: usize,
) -> Result<pop_types::StringFormatKind, MirParseError> {
    if text == "Boolean" {
        return Ok(pop_types::StringFormatKind::Boolean);
    }
    if let Some(kind) = text
        .strip_prefix("Integer(")
        .and_then(|kind| kind.strip_suffix(')'))
    {
        return parse_integer_kind(kind, line).map(pop_types::StringFormatKind::Integer);
    }
    if let Some(kind) = text
        .strip_prefix("Float(")
        .and_then(|kind| kind.strip_suffix(')'))
    {
        return parse_float_kind(kind, line).map(pop_types::StringFormatKind::Float);
    }
    Err(error(line, "string format kind"))
}

fn parse_float_kind(text: &str, line: usize) -> Result<FloatKind, MirParseError> {
    match text {
        "Float32" => Ok(FloatKind::Float32),
        "Float64" => Ok(FloatKind::Float64),
        _ => Err(error(line, "float kind")),
    }
}

fn parse_types(text: &str, line: usize) -> Result<Vec<TypeId>, MirParseError> {
    comma_parts(text)
        .map(|part| parse_prefixed(part, 't', line).map(TypeId::from_raw))
        .collect()
}

fn parse_values(text: &str, line: usize) -> Result<Vec<ValueId>, MirParseError> {
    let inner = text
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .ok_or_else(|| error(line, "malformed value list"))?;
    comma_parts(inner)
        .map(|part| parse_prefixed(part, 'v', line).map(ValueId::from_raw))
        .collect()
}

fn parse_two_values(text: &str, line: usize) -> Result<(ValueId, ValueId), MirParseError> {
    let parts: Vec<_> = text.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(error(line, "expected two operands"));
    }
    Ok((
        ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?),
        ValueId::from_raw(parse_prefixed(parts[1], 'v', line)?),
    ))
}

fn parse_three_values(
    text: &str,
    line: usize,
) -> Result<(ValueId, ValueId, ValueId), MirParseError> {
    let parts: Vec<_> = text.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(error(line, "expected three operands"));
    }
    Ok((
        ValueId::from_raw(parse_prefixed(parts[0], 'v', line)?),
        ValueId::from_raw(parse_prefixed(parts[1], 'v', line)?),
        ValueId::from_raw(parse_prefixed(parts[2], 'v', line)?),
    ))
}

fn parse_table_entries(text: &str, line: usize) -> Result<Vec<(ValueId, ValueId)>, MirParseError> {
    let inner = text
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .ok_or_else(|| error(line, "malformed table entries"))?;
    comma_parts(inner)
        .map(|entry| {
            let (key, value) = entry
                .split_once(" => ")
                .ok_or_else(|| error(line, "malformed table entry"))?;
            Ok((
                ValueId::from_raw(parse_prefixed(key, 'v', line)?),
                ValueId::from_raw(parse_prefixed(value, 'v', line)?),
            ))
        })
        .collect()
}

fn comma_parts(text: &str) -> impl Iterator<Item = &str> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

fn split_number(text: &str, line: usize) -> Result<(u32, &str), MirParseError> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    Ok((parse_u32(&text[..end], line)?, &text[end..]))
}

fn parse_prefixed(text: &str, prefix: char, line: usize) -> Result<u32, MirParseError> {
    parse_u32(
        text.strip_prefix(prefix)
            .ok_or_else(|| error(line, "invalid ID prefix"))?,
        line,
    )
}

fn parse_builtin_type_id(text: &str, line: usize) -> Result<BuiltinTypeId, MirParseError> {
    let value = text
        .strip_prefix("bt")
        .ok_or_else(|| error(line, "built-in type identity"))?;
    Ok(BuiltinTypeId::from_raw(parse_u32(value, line)?))
}

fn parse_named_prefix(text: &str, prefix: &str, line: usize) -> Result<u32, MirParseError> {
    text.strip_prefix(prefix)
        .ok_or_else(|| error(line, "identifier prefix"))
        .and_then(|value| parse_u32(value, line))
}

fn parse_hash(text: &str, prefix: &str, line: usize) -> Result<u32, MirParseError> {
    parse_u32(
        text.strip_prefix(prefix)
            .ok_or_else(|| error(line, "invalid entity ID"))?,
        line,
    )
}

fn parse_u32(text: &str, line: usize) -> Result<u32, MirParseError> {
    text.parse().map_err(|_| error(line, "invalid integer ID"))
}

fn parse_string(text: &str, line: usize) -> Result<String, MirParseError> {
    let inner = text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .ok_or_else(|| error(line, "malformed string"))?;
    let mut output = String::new();
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        output.push(
            match characters
                .next()
                .ok_or_else(|| error(line, "string escape"))?
            {
                '\\' => '\\',
                '"' => '"',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                _ => return Err(error(line, "unsupported string escape")),
            },
        );
    }
    Ok(output)
}

fn required<'a>(
    parts: &mut impl Iterator<Item = &'a str>,
    line: usize,
) -> Result<&'a str, MirParseError> {
    parts.next().ok_or_else(|| error(line, "missing component"))
}

fn empty_span() -> SourceSpan {
    SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)))
}

const fn error(line: usize, reason: &'static str) -> MirParseError {
    MirParseError { line, reason }
}
