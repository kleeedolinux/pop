//! Verified-MIR execution engine and its public resource-limited API.
//!
//! Construction verifies the complete `MirBubble` before retaining it. Execution
//! consumes resolved stable IDs only and delegates every runtime operation through
//! the backend-neutral PLRI adapter.
use crate::evaluation::*;
use crate::runtime::ReferenceRuntimeAdapter;
use crate::values::{MirClassValue, MirValue, RuntimeValue};
use pop_foundation::{NestedFunctionId, SymbolId, SymbolIdentity, TypeId, ValueId};
use pop_mir::{
    MirBubble, MirInstruction, MirInstructionKind, MirTerminator, MirVerificationError,
    verify_mir_bubble,
};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, BarrierKind, ObjectAllocationRequest, ObjectMap,
    ObjectSlot, PinHandle, RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure,
    RuntimeTypeId, TableAllocationRequest, Trap, TrapKind, WriteBarrier,
};
use pop_types::{IntegerKind, IntegerValue, PrimitiveType, SemanticType, TypeArena};
use std::cell::{Ref, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

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
        )
    )
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

pub struct MirInterpreter<'mir, R = ReferenceRuntimeAdapter> {
    mir: &'mir MirBubble,
    arena: &'mir TypeArena,
    limits: ExecutionLimits,
    runtime: RefCell<R>,
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
        })
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = limits;
        self
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
        Engine {
            mir: self.mir,
            arena: self.arena,
            limits: self.limits,
            steps: 0,
            depth: 0,
            runtime: &mut *runtime,
            root_handles: BTreeMap::new(),
            pin_handles: BTreeMap::new(),
            private_values: BTreeMap::new(),
            next_private_value: u32::MAX,
            active_captures: None,
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
    root_handles: BTreeMap<ValueId, RootHandle>,
    pin_handles: BTreeMap<ValueId, PinHandle>,
    private_values: BTreeMap<SymbolId, PrivateValue>,
    next_private_value: u32,
    active_captures: Option<Rc<RefCell<Vec<RuntimeValue>>>>,
}

enum PrivateValue {
    Cell(Rc<RefCell<RuntimeValue>>),
    Closure {
        function: NestedFunctionId,
        captures: Rc<RefCell<Vec<RuntimeValue>>>,
    },
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
                    Err(error @ ExecutionError::Runtime(RuntimeFailure::Unwind(_))) => {
                        if let Some(target) = call_cleanup_target(instruction.kind()) {
                            unwound_to_cleanup = Some(target.raw() as usize);
                            break;
                        }
                        return Err(error);
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
                    return Err(ExecutionError::Runtime(
                        self.runtime.begin_panic(payload.clone()),
                    ));
                }
                MirTerminator::ContinueUnwind(reason) => {
                    return Err(ExecutionError::Runtime(RuntimeFailure::Unwind(
                        reason.clone(),
                    )));
                }
                MirTerminator::Unreachable => return Err(ExecutionError::ReachedUnreachable),
                MirTerminator::Missing => return Err(ExecutionError::InvalidControlFlow),
            }
        }
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
            MirInstructionKind::EnumConstant {
                definition,
                case,
                discriminant,
            } => MirValue::Enum {
                definition: *definition,
                case: *case,
                discriminant: *discriminant,
            },
            MirInstructionKind::FunctionReference(function) => MirValue::Function(*function),
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
            | MirInstructionKind::CallReferenced { .. }
            | MirInstructionKind::CallDirectMethod { .. }
            | MirInstructionKind::CallInterface { .. }
            | MirInstructionKind::CallIndirect { .. }
            | MirInstructionKind::RecordMake { .. }
            | MirInstructionKind::ClassMake { .. }
            | MirInstructionKind::RecordUpdate { .. }
            | MirInstructionKind::FieldGet { .. }
            | MirInstructionKind::FieldSet { .. }
            | MirInstructionKind::UnionMake { .. }
            | MirInstructionKind::InterfaceUpcast { .. }
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
            | MirInstructionKind::Pin { .. }
            | MirInstructionKind::Unpin { .. }
            | MirInstructionKind::WriteBarrier { .. } => {
                return Err(ExecutionError::InvalidControlFlow);
            }
        };
        Ok(RuntimeValue::visible(result))
    }

    fn evaluate_effect_instruction(
        &mut self,
        instruction: &pop_mir::MirInstruction,
        values: &mut BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<(), ExecutionError> {
        let returned = match instruction.kind() {
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
                for (root, (_, relocated)) in roots.iter().copied().zip(publication.root_values()) {
                    values
                        .get_mut(&root)
                        .ok_or(ExecutionError::MissingValue(root))?
                        .install_relocated_reference(relocated)?;
                }
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
            } => {
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
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Option<RuntimeValue>, ExecutionError> {
        let result = match instruction.kind() {
            MirInstructionKind::CallDirect {
                function,
                arguments,
                ..
            } => single_result(self.execute_direct_call(*function, arguments, values)?),
            MirInstructionKind::CallReferenced { function, .. } => {
                return Err(ExecutionError::UnknownReferencedFunction(*function));
            }
            MirInstructionKind::CallDirectMethod {
                method, arguments, ..
            } => single_result(self.execute_method_call(*method, arguments, values)?),
            MirInstructionKind::CallIndirect {
                callee, arguments, ..
            } => single_result(self.execute_indirect_call(*callee, arguments, values)?),
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
                    Some(PrivateValue::Closure { .. }) => Ok(captured),
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
            MirInstructionKind::InterfaceUpcast { value: base, .. } => {
                Ok(value(values, *base)?.clone())
            }
            _ => return Ok(None),
        }?;
        Ok(Some(result))
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
        let MirValue::Function(function) = &value(values, callee)?.visible else {
            return Err(ExecutionError::TypeMismatch);
        };
        let arguments = evaluated_arguments(arguments, values)?;
        let closure = match self.private_values.get(function) {
            Some(PrivateValue::Closure { function, captures }) => {
                Some((*function, captures.clone()))
            }
            _ => None,
        };
        if let Some((function, captures)) = closure {
            let nested = self
                .mir
                .nested_functions()
                .iter()
                .find(|candidate| candidate.function() == function)
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
                &arguments,
                Some(captures),
            );
            self.depth -= 1;
            result
        } else {
            self.call(*function, &arguments)
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
