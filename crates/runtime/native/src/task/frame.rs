// Native task and cancellation ABI implementation.

use crate::failure::pop_rt_trap;
use crate::{
    SchedulerTask, SchedulerTaskContext, SchedulerTaskFrame, SchedulerTaskFrameError,
    SchedulerTaskPoll,
};
use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::{
    CancellationObservation, CancellationTokenId, SchedulerId, TaskGroupExit, TaskGroupId,
    TaskGroupLifecycle, TaskId, TaskLifecycle, TaskOwner, TaskPollCompletion, TaskState,
};
use pop_runtime_interface::{
    ManagedReference, RootHandle, RootPublication, RootSlot, RuntimeAdapter, SafePointId, StackMap,
};
use pop_runtime_native_abi::NativeTaskStatus;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTaskFrameError {
    InvalidRootMap,
    RootOutOfBounds(RootSlot),
    UnknownSlot(u32),
    PublicationShape,
}

/// LLVM-private coroutine storage exposed to the native scheduler through an
/// exact backend-neutral stack map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTaskFrame {
    slots: Vec<u64>,
    stack_map: StackMap,
}

fn native_task_frame_pointer(handle: u64) -> Option<*mut NativeTaskFrame> {
    let address = usize::try_from(handle).ok()?;
    std::ptr::NonNull::new(address as *mut NativeTaskFrame).map(std::ptr::NonNull::as_ptr)
}

impl NativeTaskFrame {
    /// Constructs one compiler-created frame at its initial ready safe point.
    ///
    /// # Errors
    ///
    /// Rejects duplicate or out-of-bounds root slots.
    pub fn new(
        slots: Vec<u64>,
        safe_point: SafePointId,
        root_slots: Vec<RootSlot>,
    ) -> Result<Self, NativeTaskFrameError> {
        let stack_map = Self::validated_stack_map(slots.len(), safe_point, root_slots)?;
        Ok(Self { slots, stack_map })
    }

    fn validated_stack_map(
        slot_count: usize,
        safe_point: SafePointId,
        root_slots: Vec<RootSlot>,
    ) -> Result<StackMap, NativeTaskFrameError> {
        let stack_map = StackMap::new(safe_point, root_slots)
            .map_err(|_| NativeTaskFrameError::InvalidRootMap)?;
        if let Some(root) = stack_map
            .root_slots()
            .iter()
            .copied()
            .find(|root| root.raw() as usize >= slot_count)
        {
            return Err(NativeTaskFrameError::RootOutOfBounds(root));
        }
        Ok(stack_map)
    }

    #[must_use]
    pub const fn safe_point(&self) -> SafePointId {
        self.stack_map.safe_point()
    }

    #[must_use]
    pub fn stack_map(&self) -> &StackMap {
        &self.stack_map
    }

    /// Replaces the current resume state and its exact compiler-proven live
    /// roots before a nonterminal scheduler poll is committed.
    ///
    /// # Errors
    ///
    /// Rejects duplicate or out-of-bounds root slots without changing the
    /// previous live map.
    pub fn set_live_frame(
        &mut self,
        safe_point: SafePointId,
        root_slots: Vec<RootSlot>,
    ) -> Result<(), NativeTaskFrameError> {
        let stack_map = Self::validated_stack_map(self.slots.len(), safe_point, root_slots)?;
        self.stack_map = stack_map;
        Ok(())
    }

    /// Reads one compiler-known physical frame slot.
    ///
    /// # Errors
    ///
    /// Rejects an index outside the allocated frame.
    pub fn slot(&self, slot: u32) -> Result<u64, NativeTaskFrameError> {
        self.slots
            .get(slot as usize)
            .copied()
            .ok_or(NativeTaskFrameError::UnknownSlot(slot))
    }

    /// Writes one compiler-known physical frame slot.
    ///
    /// # Errors
    ///
    /// Rejects an index outside the allocated frame.
    pub fn set_slot(&mut self, slot: u32, value: u64) -> Result<(), NativeTaskFrameError> {
        let stored = self
            .slots
            .get_mut(slot as usize)
            .ok_or(NativeTaskFrameError::UnknownSlot(slot))?;
        *stored = value;
        Ok(())
    }

    fn publication(&self) -> Result<RootPublication, NativeTaskFrameError> {
        let values = self
            .stack_map
            .root_slots()
            .iter()
            .map(|root| {
                self.slots
                    .get(root.raw() as usize)
                    .copied()
                    .ok_or(NativeTaskFrameError::RootOutOfBounds(*root))
                    .map(|value| (value != 0).then(|| ManagedReference::new(value)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        RootPublication::new(self.stack_map.clone(), values)
            .map_err(|_| NativeTaskFrameError::PublicationShape)
    }

    fn restore(&mut self, publication: &RootPublication) -> Result<(), NativeTaskFrameError> {
        if publication.stack_map() != &self.stack_map {
            return Err(NativeTaskFrameError::PublicationShape);
        }
        for (root, value) in publication.root_values() {
            let stored = self
                .slots
                .get_mut(root.raw() as usize)
                .ok_or(NativeTaskFrameError::RootOutOfBounds(root))?;
            *stored = value.map_or(0, ManagedReference::raw);
        }
        Ok(())
    }
}

/// One compiler-generated stackless poll callback plus its exact native frame.
pub struct NativeCompilerTask<P> {
    frame: NativeTaskFrame,
    poll: P,
}

impl<P> NativeCompilerTask<P> {
    #[must_use]
    pub const fn new(frame: NativeTaskFrame, poll: P) -> Self {
        Self { frame, poll }
    }

    #[must_use]
    pub const fn frame(&self) -> &NativeTaskFrame {
        &self.frame
    }

    #[must_use]
    pub const fn frame_mut(&mut self) -> &mut NativeTaskFrame {
        &mut self.frame
    }
}

impl<P: Send + 'static> SchedulerTaskFrame for NativeCompilerTask<P> {
    fn frame_stack_map(&self) -> StackMap {
        self.frame.stack_map.clone()
    }

    fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
        self.frame
            .publication()
            .map_err(|_| SchedulerTaskFrameError::PublicationRejected)
    }

    fn restore_frame_roots(
        &mut self,
        publication: RootPublication,
    ) -> Result<(), SchedulerTaskFrameError> {
        self.frame
            .restore(&publication)
            .map_err(|_| SchedulerTaskFrameError::RestorationRejected)
    }
}

impl<P> SchedulerTask for NativeCompilerTask<P>
where
    P: FnMut(&mut NativeTaskFrame, &SchedulerTaskContext) -> SchedulerTaskPoll + Send + 'static,
{
    fn poll(&mut self, context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        (self.poll)(&mut self.frame, context)
    }
}
