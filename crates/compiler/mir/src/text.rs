use std::error::Error;
use std::fmt;

use pop_foundation::{
    BindingId, BlockId, BubbleId, CaptureId, ClassId, FieldId, FileId, FunctionId, InterfaceId,
    InterfaceMethodId, MethodId, NamespaceId, NestedFunctionId, SourceSpan, SymbolId, TextRange,
    TextSize, TypeId, UnionCaseId, ValueId,
};
use pop_runtime_interface::{
    ArrayElementMap, ObjectMap, ObjectSlot, PanicKind, PanicPayload, RootSlot, SafePointId,
    StackMap, Trap, TrapKind, UnwindReason,
};
use pop_types::{FloatKind, FloatValue, IntegerKind, IntegerValue};

use super::{
    MirBlock, MirBlockArgument, MirBubble, MirCapture, MirCaptureMode, MirClassDeclaration,
    MirDeclaration, MirDeclarationKind, MirEffect, MirEffectSummary, MirField, MirFunction,
    MirInstruction, MirInstructionKind, MirInterfaceDeclaration, MirInterfaceImplementation,
    MirInterfaceMethod, MirInterfaceMethodImplementation, MirMethod, MirNestedFunction,
    MirRecordDeclaration, MirTerminator, MirUnionCase, MirUnionDeclaration, MirUnionSwitchArm,
    MirUnwindAction, local_instruction_effects,
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
    while position < lines.len() {
        if lines[position].starts_with("type.") {
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
        } else if lines[position].starts_with("nested ") {
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
    let header = lines[start];
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
    let parts: Vec<_> = lines[start].split_whitespace().collect();
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

fn parse_block(lines: &[&str], start: usize) -> Result<(MirBlock, usize), MirParseError> {
    let number = start + 1;
    let header = lines[start]
        .trim()
        .strip_prefix('b')
        .ok_or_else(|| error(number, "expected block"))?;
    let (block, rest) = split_number(header, number)?;
    let arguments = rest
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix("):"))
        .ok_or_else(|| error(number, "malformed block arguments"))?;
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
            arguments,
            instructions,
            terminator,
        },
        position + 1,
    ))
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
        let kind = parse_operation(operation, line)?;
        if !matches!(
            kind,
            MirInstructionKind::CallDirect { .. }
                | MirInstructionKind::CallDirectMethod { .. }
                | MirInstructionKind::CallInterface { .. }
                | MirInstructionKind::CallIndirect { .. }
                | MirInstructionKind::GcSafePoint { .. }
                | MirInstructionKind::RetainRoot { .. }
                | MirInstructionKind::ReleaseRoot { .. }
                | MirInstructionKind::WriteBarrier { .. }
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
            span: empty_span(),
        });
    }
    let (result, operation) = text
        .split_once(" = ")
        .ok_or_else(|| error(line, "malformed instruction"))?;
    let (value, type_id) = result
        .split_once(':')
        .ok_or_else(|| error(line, "malformed result"))?;
    let kind = parse_operation(operation, line)?;
    let effects = local_instruction_effects(&kind);
    Ok(MirInstruction {
        result: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        result_type: Some(TypeId::from_raw(parse_prefixed(type_id, 't', line)?)),
        kind,
        effects,
        effects_explicit: true,
        span: empty_span(),
    })
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
    if let Some(values) = text.strip_prefix("tupleMake ") {
        return Ok(MirInstructionKind::TupleMake(parse_values(values, line)?));
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
    if let Some(rest) = text.strip_prefix("tableMake ") {
        let (object_map, entries) = rest
            .split_once(' ')
            .ok_or_else(|| error(line, "table allocation map"))?;
        return Ok(MirInstructionKind::TableMake {
            entries: parse_table_entries(entries, line)?,
            object_map: parse_object_map(object_map, line)?,
        });
    }
    if let Some(operands) = text.strip_prefix("arrayGet ") {
        let (array, index) = parse_two_values(operands, line)?;
        return Ok(MirInstructionKind::ArrayGet { array, index });
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
    if text.starts_with("callDirect")
        || text.starts_with("callIndirect")
        || text.starts_with("call.interface")
    {
        return parse_call_operation(text, line);
    }
    if let Some(rest) = text.strip_prefix("interface.upcast ") {
        let mut parts = rest.split_whitespace();
        return Ok(MirInstructionKind::InterfaceUpcast {
            value: ValueId::from_raw(parse_prefixed(required(&mut parts, line)?, 'v', line)?),
            interface: InterfaceId::from_raw(parse_prefixed(
                required(&mut parts, line)?,
                'i',
                line,
            )?),
        });
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
            value: ValueId::from_raw(parse_prefixed(value, 'v', line)?),
        });
    }
    if let Some(rest) = text.strip_prefix("writeBarrier ") {
        return parse_write_barrier(rest, line);
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
    Ok(None)
}

fn parse_call_operation(text: &str, line: usize) -> Result<MirInstructionKind, MirParseError> {
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
            "MayTrap" => Ok(MirEffect::MayTrap),
            "MayUnwind" => Ok(MirEffect::MayUnwind),
            "Suspends" => Ok(MirEffect::Suspends),
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
    Err(error(line, "unknown terminator"))
}

fn parse_trap_kind(text: &str, line: usize) -> Result<TrapKind, MirParseError> {
    match text {
        "IntegerOverflow" => Ok(TrapKind::IntegerOverflow),
        "DivisionByZero" => Ok(TrapKind::DivisionByZero),
        "BoundsViolation" => Ok(TrapKind::BoundsViolation),
        "ImpossibleState" => Ok(TrapKind::ImpossibleState),
        _ => Err(error(line, "trap kind")),
    }
}

fn parse_panic_payload(text: &str, line: usize) -> Result<PanicPayload, MirParseError> {
    if text == "RuntimeInvariant" {
        return Ok(PanicPayload::new(PanicKind::RuntimeInvariant));
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
    if !(text.starts_with("integer.") || text.starts_with("float.")) {
        return Ok(None);
    }
    let parts = text.split_whitespace().collect::<Vec<_>>();
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
        "integer.compareGreater" => MirInstructionKind::CompareIntegerGreater {
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
        "float.compareGreater" => MirInstructionKind::CompareFloatGreater {
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
