//! Reference interpreter for verified canonical MIR.
#![allow(clippy::too_many_lines)]

use std::cell::{Ref, RefCell};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::rc::Rc;

use pop_foundation::{ClassId, FieldId, NestedFunctionId, SymbolId, UnionCaseId, ValueId};
use pop_mir::{
    MirBubble, MirFunction, MirInstruction, MirInstructionKind, MirTerminator, MirUnwindAction,
    MirVerificationError, verify_mir_bubble,
};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, BarrierKind,
    GarbageCollectorContract, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, RuntimeTypeId, SafePointId,
    SafePointOutcome, Trap, TrapKind, WriteBarrier,
};
use pop_types::{
    FloatKind, FloatValue, IntegerKind, IntegerValue, NumericError, PrimitiveType, SemanticType,
    TypeArena,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirValue {
    Nil,
    Boolean(bool),
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
    Tuple(Vec<Self>),
    Array(Vec<Self>),
    Table(Vec<(Self, Self)>),
    Function(SymbolId),
    Record {
        record: SymbolId,
        fields: Vec<(FieldId, Self)>,
    },
    Class(MirClassValue),
    Union {
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<Self>,
    },
}

#[derive(Clone, Debug)]
pub struct MirClassValue {
    class: ClassId,
    reference: ManagedReference,
    fields: Rc<RefCell<Vec<(FieldId, RuntimeValue)>>>,
}

impl MirClassValue {
    fn new(
        class: ClassId,
        reference: ManagedReference,
        fields: Vec<(FieldId, RuntimeValue)>,
    ) -> Self {
        Self {
            class,
            reference,
            fields: Rc::new(RefCell::new(fields)),
        }
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }
}

#[derive(Clone, Debug)]
struct RuntimeValue {
    visible: MirValue,
    reference: Option<ManagedReference>,
}

impl RuntimeValue {
    fn visible(visible: MirValue) -> Self {
        let reference = match &visible {
            MirValue::Class(class) => Some(class.reference),
            _ => None,
        };
        Self { visible, reference }
    }

    const fn managed(visible: MirValue, reference: ManagedReference) -> Self {
        Self {
            visible,
            reference: Some(reference),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReferenceRuntimeEvent {
    AllocateObject {
        type_id: RuntimeTypeId,
        object_map: ObjectMap,
    },
    AllocateArray {
        type_id: RuntimeTypeId,
        length: u32,
        element_map: ArrayElementMap,
    },
    RetainRoot(ManagedReference),
    ReleaseRoot(RootHandle),
    SafePoint {
        safe_point: SafePointId,
        roots: Vec<ManagedReference>,
    },
    WriteBarrier(WriteBarrier),
    Trap(Trap),
    Panic(pop_runtime_interface::PanicPayload),
}

#[derive(Default)]
pub struct ReferenceRuntimeAdapter {
    allocations: BTreeMap<ManagedReference, ObjectMap>,
    roots: BTreeMap<RootHandle, ManagedReference>,
    next_reference: u64,
    next_root: u64,
    events: Vec<ReferenceRuntimeEvent>,
}

impl ReferenceRuntimeAdapter {
    #[must_use]
    pub fn events(&self) -> &[ReferenceRuntimeEvent] {
        &self.events
    }

    fn allocate_map(&mut self, map: ObjectMap) -> ManagedReference {
        self.next_reference = self.next_reference.saturating_add(1).max(1);
        let reference = ManagedReference::new(self.next_reference);
        self.allocations.insert(reference, map);
        reference
    }

    fn valid_reference(&self, reference: ManagedReference) -> Result<(), RuntimeFailure> {
        if self.allocations.contains_key(&reference) {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }
}

impl RuntimeAdapter for ReferenceRuntimeAdapter {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::bootstrap_stage1()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.events.push(ReferenceRuntimeEvent::AllocateObject {
            type_id: request.type_id(),
            object_map: request.object_map().clone(),
        });
        Ok(self.allocate_map(request.object_map().clone()))
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.events.push(ReferenceRuntimeEvent::AllocateArray {
            type_id: request.type_id(),
            length: request.length(),
            element_map: request.element_map(),
        });
        let references = match request.element_map() {
            ArrayElementMap::Scalar => Vec::new(),
            ArrayElementMap::ManagedReference => {
                (0..request.length()).map(ObjectSlot::new).collect()
            }
        };
        let map = ObjectMap::new(request.length(), references)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        Ok(self.allocate_map(map))
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.valid_reference(reference)?;
        self.events
            .push(ReferenceRuntimeEvent::RetainRoot(reference));
        self.next_root = self.next_root.saturating_add(1).max(1);
        let root = RootHandle::new(self.next_root);
        self.roots.insert(root, reference);
        Ok(root)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        let result = self
            .roots
            .remove(&root)
            .map(|_| ())
            .ok_or_else(RuntimeFailure::runtime_invariant);
        if result.is_ok() {
            self.events.push(ReferenceRuntimeEvent::ReleaseRoot(root));
        }
        result
    }

    fn safe_point(&mut self, roots: &RootPublication) -> Result<SafePointOutcome, RuntimeFailure> {
        for reference in roots.managed_references() {
            self.valid_reference(reference)?;
        }
        self.events.push(ReferenceRuntimeEvent::SafePoint {
            safe_point: roots.stack_map().safe_point(),
            roots: roots.managed_references().collect(),
        });
        Ok(SafePointOutcome::no_collection())
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.valid_reference(barrier.owner())?;
        if let Some(previous) = barrier.previous() {
            self.valid_reference(previous)?;
        }
        if let Some(value) = barrier.value() {
            self.valid_reference(value)?;
        }
        if !self
            .allocations
            .get(&barrier.owner())
            .is_some_and(|map| map.is_reference_slot(barrier.slot()))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.events
            .push(ReferenceRuntimeEvent::WriteBarrier(barrier));
        Ok(())
    }

    fn raise_trap(&mut self, trap: Trap) -> RuntimeFailure {
        self.events.push(ReferenceRuntimeEvent::Trap(trap));
        RuntimeFailure::Trap(trap)
    }

    fn begin_panic(&mut self, payload: pop_runtime_interface::PanicPayload) -> RuntimeFailure {
        self.events
            .push(ReferenceRuntimeEvent::Panic(payload.clone()));
        RuntimeFailure::from_panic(payload)
    }
}

impl PartialEq for MirClassValue {
    fn eq(&self, other: &Self) -> bool {
        self.class == other.class && Rc::ptr_eq(&self.fields, &other.fields)
    }
}

impl Eq for MirClassValue {}

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
    WrongArity,
    TypeMismatch,
    MissingValue(ValueId),
    IntegerOverflow,
    DivisionByZero,
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
            retained_roots: BTreeMap::new(),
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
    retained_roots: BTreeMap<ManagedReference, Vec<RootHandle>>,
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
        parameters: &[pop_types::TypeId],
        results: &[pop_types::TypeId],
        blocks: &[pop_mir::MirBlock],
        arguments: &[RuntimeValue],
        captures: Option<Rc<RefCell<Vec<RuntimeValue>>>>,
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        require_runtime_numeric_types(self.arena, parameters, arguments)?;
        let previous_captures = self.active_captures.replace(captures);
        let result = self.execute_blocks(results, blocks, arguments);
        self.active_captures = previous_captures;
        result
    }

    fn execute_blocks(
        &mut self,
        results: &[pop_types::TypeId],
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
            let block = blocks.get(block_index).ok_or(ExecutionError::InvalidControlFlow)?;
            let mut unwound_to_cleanup = None;
            for instruction in block.instructions() {
                self.step()?;
                let evaluated = if instruction.has_result() {
                    self.evaluate_instruction(instruction, &values).map(Some)
                } else {
                    self.evaluate_effect_instruction(instruction.kind(), &values)
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
        values: &BTreeMap<ValueId, RuntimeValue>,
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
            Err(error) => return Err(error),
        }
        let result = match instruction.kind() {
            MirInstructionKind::StringConstant(value) => MirValue::String(value.clone()),
            MirInstructionKind::BooleanConstant(value) => MirValue::Boolean(*value),
            MirInstructionKind::NilConstant => MirValue::Nil,
            MirInstructionKind::FunctionReference(function) => MirValue::Function(*function),
            MirInstructionKind::TupleMake(elements) => MirValue::Tuple(
                elements
                    .iter()
                    .map(|element| value(values, *element).map(|value| value.visible.clone()))
                    .collect::<Result<_, _>>()?,
            ),
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
            MirInstructionKind::TableMake {
                entries,
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
            | MirInstructionKind::CompareIntegerLess { .. }
            | MirInstructionKind::CompareIntegerGreater { .. }
            | MirInstructionKind::CompareFloatLess { .. }
            | MirInstructionKind::CompareFloatGreater { .. }
            | MirInstructionKind::CallDirect { .. }
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
            | MirInstructionKind::WriteBarrier { .. } => {
                return Err(ExecutionError::InvalidControlFlow);
            }
        };
        Ok(RuntimeValue::visible(result))
    }

    fn evaluate_effect_instruction(
        &mut self,
        instruction: &MirInstructionKind,
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<(), ExecutionError> {
        let returned = match instruction {
            MirInstructionKind::CallDirect {
                function,
                arguments,
                ..
            } => self.execute_direct_call(*function, arguments, values)?,
            MirInstructionKind::CallDirectMethod {
                method, arguments, ..
            } => self.execute_method_call(*method, arguments, values)?,
            MirInstructionKind::CallIndirect {
                callee, arguments, ..
            } => self.execute_indirect_call(*callee, arguments, values)?,
            MirInstructionKind::GcSafePoint {
                roots, stack_map, ..
            } => {
                let roots = roots
                    .iter()
                    .map(|root| value(values, *root).map(|value| value.reference))
                    .collect::<Result<_, _>>()?;
                let publication = RootPublication::new(stack_map.clone(), roots)
                    .map_err(|_| ExecutionError::InvalidControlFlow)?;
                self.runtime
                    .safe_point(&publication)
                    .map_err(ExecutionError::Runtime)?;
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
                self.retained_roots
                    .entry(reference)
                    .or_default()
                    .push(handle);
                return Ok(());
            }
            MirInstructionKind::ReleaseRoot { value: root } => {
                let reference = value(values, *root)?
                    .reference
                    .ok_or(ExecutionError::TypeMismatch)?;
                let handle = self
                    .retained_roots
                    .get_mut(&reference)
                    .and_then(Vec::pop)
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                self.runtime
                    .release_root(handle)
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
            Some(PrivateValue::Closure { function, captures }) => Some((*function, captures.clone())),
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

fn evaluate_numeric_instruction(
    instruction: &MirInstructionKind,
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Option<MirValue>, ExecutionError> {
    let result = match instruction {
        MirInstructionKind::IntegerConstant(value) => Ok(MirValue::Integer(*value)),
        MirInstructionKind::FloatConstant(value) => Ok(MirValue::Float(*value)),
        MirInstructionKind::CheckedIntegerAdd { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_add)
        }
        MirInstructionKind::CheckedIntegerSubtract { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_subtract)
        }
        MirInstructionKind::CheckedIntegerMultiply { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_multiply)
        }
        MirInstructionKind::CheckedIntegerDivide { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_divide)
        }
        MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => {
            checked_integer_binary(
                values,
                *kind,
                *left,
                *right,
                IntegerValue::checked_remainder,
            )
        }
        MirInstructionKind::FloatAdd { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_add)
        }
        MirInstructionKind::FloatSubtract { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_subtract)
        }
        MirInstructionKind::FloatMultiply { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_multiply)
        }
        MirInstructionKind::FloatDivide { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_divide)
        }
        MirInstructionKind::IntegerNegate { kind, operand } => integer(values, *kind, *operand)?
            .checked_negate()
            .map(MirValue::Integer)
            .map_err(execution_numeric_error),
        MirInstructionKind::FloatNegate { kind, operand } => Ok(MirValue::Float(
            float_value(values, *kind, *operand)?.negate(),
        )),
        MirInstructionKind::CompareIntegerLess { kind, left, right } => {
            compare_integer(values, *kind, *left, *right, Ordering::is_lt)
        }
        MirInstructionKind::CompareIntegerGreater { kind, left, right } => {
            compare_integer(values, *kind, *left, *right, Ordering::is_gt)
        }
        MirInstructionKind::CompareFloatLess { kind, left, right } => {
            compare_float(values, *kind, *left, *right, Ordering::is_lt)
        }
        MirInstructionKind::CompareFloatGreater { kind, left, right } => {
            compare_float(values, *kind, *left, *right, Ordering::is_gt)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(result))
}

fn value(
    values: &BTreeMap<ValueId, RuntimeValue>,
    id: ValueId,
) -> Result<&RuntimeValue, ExecutionError> {
    values.get(&id).ok_or(ExecutionError::MissingValue(id))
}

fn evaluated_arguments(
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Vec<RuntimeValue>, ExecutionError> {
    arguments
        .iter()
        .map(|argument| value(values, *argument).cloned())
        .collect()
}

fn single_result(mut returned: Vec<RuntimeValue>) -> Result<RuntimeValue, ExecutionError> {
    if returned.len() != 1 {
        return Err(ExecutionError::WrongArity);
    }
    returned.pop().ok_or(ExecutionError::WrongArity)
}

fn require_runtime_numeric_types(
    arena: &TypeArena,
    expected: &[pop_foundation::TypeId],
    values: &[RuntimeValue],
) -> Result<(), ExecutionError> {
    if expected.len() != values.len() {
        return Err(ExecutionError::WrongArity);
    }
    for (expected, value) in expected.iter().zip(values) {
        let matches = match arena.get(*expected) {
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
                matches!(&value.visible, MirValue::Integer(integer) if integer.kind() == *kind)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
                matches!(&value.visible, MirValue::Float(float) if float.kind() == FloatKind::Float32)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
                matches!(&value.visible, MirValue::Float(float) if float.kind() == FloatKind::Float64)
            }
            _ => true,
        };
        if !matches {
            return Err(ExecutionError::TypeMismatch);
        }
    }
    Ok(())
}

fn integer(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    operand: ValueId,
) -> Result<IntegerValue, ExecutionError> {
    match &value(values, operand)?.visible {
        MirValue::Integer(value) if value.kind() == kind => Ok(*value),
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn integers(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
) -> Result<(IntegerValue, IntegerValue), ExecutionError> {
    Ok((integer(values, kind, left)?, integer(values, kind, right)?))
}

fn checked_integer_binary(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
    operation: fn(IntegerValue, IntegerValue) -> Result<IntegerValue, NumericError>,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = integers(values, kind, left, right)?;
    operation(left, right)
        .map(MirValue::Integer)
        .map_err(execution_numeric_error)
}

fn float_value(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    operand: ValueId,
) -> Result<FloatValue, ExecutionError> {
    match &value(values, operand)?.visible {
        MirValue::Float(value) if value.kind() == kind => Ok(*value),
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn floats(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    left: ValueId,
    right: ValueId,
) -> Result<(FloatValue, FloatValue), ExecutionError> {
    Ok((
        float_value(values, kind, left)?,
        float_value(values, kind, right)?,
    ))
}

fn checked_float_binary(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    left: ValueId,
    right: ValueId,
    operation: fn(FloatValue, FloatValue) -> Result<FloatValue, NumericError>,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = floats(values, kind, left, right)?;
    operation(left, right)
        .map(MirValue::Float)
        .map_err(execution_numeric_error)
}

fn boolean_binary(
    values: &BTreeMap<ValueId, RuntimeValue>,
    left: ValueId,
    right: ValueId,
    operation: impl FnOnce(bool, bool) -> bool,
) -> Result<MirValue, ExecutionError> {
    match (
        &value(values, left)?.visible,
        &value(values, right)?.visible,
    ) {
        (MirValue::Boolean(left), MirValue::Boolean(right)) => {
            Ok(MirValue::Boolean(operation(*left, *right)))
        }
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn compare_integer(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
    comparison: impl FnOnce(Ordering) -> bool,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = integers(values, kind, left, right)?;
    let ordering = left.compare(right).map_err(execution_numeric_error)?;
    Ok(MirValue::Boolean(comparison(ordering)))
}

fn compare_float(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    left: ValueId,
    right: ValueId,
    comparison: impl FnOnce(Ordering) -> bool,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = floats(values, kind, left, right)?;
    let ordering = left
        .partial_compare(right)
        .map_err(execution_numeric_error)?;
    Ok(MirValue::Boolean(ordering.is_some_and(comparison)))
}

const fn execution_numeric_error(error: NumericError) -> ExecutionError {
    match error {
        NumericError::KindMismatch => ExecutionError::TypeMismatch,
        NumericError::Overflow | NumericError::OutOfRange => ExecutionError::IntegerOverflow,
        NumericError::DivisionByZero => ExecutionError::DivisionByZero,
        NumericError::InvalidLiteral => ExecutionError::InvalidControlFlow,
    }
}

fn pop_value_equal(left: &MirValue, right: &MirValue) -> bool {
    match (left, right) {
        (MirValue::Nil, MirValue::Nil) => true,
        (MirValue::Boolean(left), MirValue::Boolean(right)) => left == right,
        (MirValue::Integer(left), MirValue::Integer(right)) => left == right,
        (MirValue::String(left), MirValue::String(right)) => left == right,
        (MirValue::Tuple(left), MirValue::Tuple(right)) => values_equal(left, right),
        (
            MirValue::Record {
                fields: left_fields,
                ..
            },
            MirValue::Record {
                fields: right_fields,
                ..
            },
        ) => record_fields_equal(left_fields, right_fields),
        (MirValue::Class(left), MirValue::Class(right)) => left == right,
        (
            MirValue::Union {
                union: left_union,
                case: left_case,
                arguments: left_arguments,
            },
            MirValue::Union {
                union: right_union,
                case: right_case,
                arguments: right_arguments,
            },
        ) => {
            left_union == right_union
                && left_case == right_case
                && values_equal(left_arguments, right_arguments)
        }
        _ => false,
    }
}

fn values_equal(left: &[MirValue], right: &[MirValue]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| pop_value_equal(left, right))
}

fn record_fields_equal(left: &[(FieldId, MirValue)], right: &[(FieldId, MirValue)]) -> bool {
    left.len() == right.len()
        && left.iter().all(|(field, left_value)| {
            right
                .iter()
                .find(|(candidate, _)| candidate == field)
                .is_some_and(|(_, right_value)| pop_value_equal(left_value, right_value))
        })
}

fn update_record(
    record: SymbolId,
    base: ValueId,
    fields: &[(FieldId, ValueId)],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<RuntimeValue, ExecutionError> {
    let MirValue::Record {
        fields: base_fields,
        ..
    } = &value(values, base)?.visible
    else {
        return Err(ExecutionError::TypeMismatch);
    };
    let mut updated = base_fields.clone();
    for (field, value) in evaluate_visible_fields(fields, values)? {
        if let Some(existing) = updated.iter_mut().find(|(existing, _)| *existing == field) {
            existing.1 = value;
        } else {
            return Err(ExecutionError::InvalidControlFlow);
        }
    }
    Ok(RuntimeValue::visible(MirValue::Record {
        record,
        fields: updated,
    }))
}

fn get_field(
    base: ValueId,
    field: FieldId,
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<RuntimeValue, ExecutionError> {
    match &value(values, base)?.visible {
        MirValue::Record { fields, .. } => {
            find_visible_field(fields, field).map(RuntimeValue::visible)
        }
        MirValue::Class(class) => find_runtime_field(&class.fields.borrow(), field),
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn find_visible_field(
    fields: &[(FieldId, MirValue)],
    field: FieldId,
) -> Result<MirValue, ExecutionError> {
    fields
        .iter()
        .find(|(candidate, _)| *candidate == field)
        .map(|(_, value)| value.clone())
        .ok_or(ExecutionError::InvalidControlFlow)
}

fn find_runtime_field(
    fields: &[(FieldId, RuntimeValue)],
    field: FieldId,
) -> Result<RuntimeValue, ExecutionError> {
    fields
        .iter()
        .find(|(candidate, _)| *candidate == field)
        .map(|(_, value)| value.clone())
        .ok_or(ExecutionError::InvalidControlFlow)
}

fn set_field(
    base: ValueId,
    field: FieldId,
    new_value: ValueId,
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<RuntimeValue, ExecutionError> {
    let MirValue::Class(class) = &value(values, base)?.visible else {
        return Err(ExecutionError::TypeMismatch);
    };
    let new_value = value(values, new_value)?.clone();
    let mut fields = class.fields.borrow_mut();
    let Some((_, current)) = fields.iter_mut().find(|(candidate, _)| *candidate == field) else {
        return Err(ExecutionError::InvalidControlFlow);
    };
    *current = new_value;
    Ok(RuntimeValue::visible(MirValue::Nil))
}

fn evaluate_fields(
    fields: &[(FieldId, ValueId)],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Vec<(FieldId, RuntimeValue)>, ExecutionError> {
    fields
        .iter()
        .map(|(field, value_id)| Ok((*field, value(values, *value_id)?.clone())))
        .collect()
}

fn evaluate_visible_fields(
    fields: &[(FieldId, ValueId)],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Vec<(FieldId, MirValue)>, ExecutionError> {
    fields
        .iter()
        .map(|(field, value_id)| Ok((*field, value(values, *value_id)?.visible.clone())))
        .collect()
}

fn call_cleanup_target(instruction: &MirInstructionKind) -> Option<pop_foundation::BlockId> {
    match instruction {
        MirInstructionKind::CallDirect {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        }
        | MirInstructionKind::CallDirectMethod {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        }
        | MirInstructionKind::CallIndirect {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        } => Some(*target),
        _ => None,
    }
}
