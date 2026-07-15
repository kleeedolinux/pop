//! Verified-MIR execution engine and its public resource-limited API.
//!
//! Construction verifies the complete `MirBubble` before retaining it. Execution
//! consumes resolved stable IDs only and delegates every runtime operation through
//! the backend-neutral PLRI adapter.
use crate::evaluation::*;
use crate::ffi_buffer::{integer_from_u64, integer_i64, integer_u64, marshal, unmarshal};
use crate::runtime::ReferenceRuntimeAdapter;
use crate::values::{MirClassValue, MirValue, RuntimeValue};
use pop_foundation::{BorrowRegionId, NestedFunctionId, SymbolId, SymbolIdentity, TypeId, ValueId};
use pop_mir::{
    MirBubble, MirCancellationMode, MirInstruction, MirInstructionKind, MirSuspendOperation,
    MirTaskDispatch, MirTerminator, MirUnwindAction, MirVerificationError, verify_mir_bubble,
};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, BarrierKind, CancellationObservation,
    CancellationTokenId, FfiBufferBorrowId, FfiBufferOpenFailure, FfiBufferOpenRequest,
    ForeignAddress, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot, PinHandle,
    RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, RuntimeTypeId,
    TableAllocationRequest, TaskGroupExit, TaskGroupId, TaskGroupLifecycle, TaskId, TaskLifecycle,
    TaskOwner, TaskPollCompletion, TaskState as RuntimeTaskState, Trap, TrapKind, UnwindReason,
    WriteBarrier,
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
            ffi_handles: BTreeMap::new(),
            ffi_buffer_borrows: BTreeMap::new(),
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
    root_handles: BTreeMap<ValueId, RootHandle>,
    ffi_handles: BTreeMap<RootHandle, RuntimeValue>,
    ffi_buffer_borrows: BTreeMap<BorrowRegionId, FfiBufferBorrowId>,
    pin_handles: BTreeMap<ValueId, PinHandle>,
    private_values: BTreeMap<SymbolId, PrivateValue>,
    next_private_value: u32,
    active_captures: Option<Rc<RefCell<Vec<RuntimeValue>>>>,
    active_task: Option<TaskId>,
}

enum PrivateValue {
    Cell(Rc<RefCell<RuntimeValue>>),
    Closure {
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
        for (root, (_, relocated)) in frame
            .stack_map()
            .root_slots()
            .iter()
            .zip(publication.root_values())
        {
            let slot = frame
                .slots()
                .get(root.raw() as usize)
                .ok_or(ExecutionError::InvalidControlFlow)?;
            values
                .get_mut(&slot.value())
                .ok_or(ExecutionError::MissingValue(slot.value()))?
                .install_relocated_reference(relocated)?;
        }
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
            | MirInstructionKind::FfiBufferWrite { .. }
            | MirInstructionKind::FfiBufferEndBorrow { .. }
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
            MirInstructionKind::CallForeign { function, .. } => {
                return Err(ExecutionError::UnsupportedForeignFunction(*function));
            }
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
        values: &BTreeMap<ValueId, RuntimeValue>,
    ) -> Result<Option<RuntimeValue>, ExecutionError> {
        let result = match instruction.kind() {
            MirInstructionKind::CallDirect {
                function,
                arguments,
                ..
            } => single_result(self.execute_direct_call(*function, arguments, values)?),
            MirInstructionKind::CallForeign { function, .. } => {
                return Err(ExecutionError::UnsupportedForeignFunction(*function));
            }
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

    fn execute_indirect_value(
        &mut self,
        callee: &RuntimeValue,
        arguments: &[RuntimeValue],
    ) -> Result<Vec<RuntimeValue>, ExecutionError> {
        let MirValue::Function(function) = &callee.visible else {
            return Err(ExecutionError::TypeMismatch);
        };
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
