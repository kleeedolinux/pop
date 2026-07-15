//! Backend-neutral task ownership, cancellation, and coroutine lifecycle.

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TaskId(u64);

impl TaskId {
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TaskGroupId(u64);

impl TaskGroupId {
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CancellationTokenId(u64);

impl CancellationTokenId {
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOwner {
    DirectAwait { parent: Option<TaskId> },
    Group(TaskGroupId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Created,
    Ready,
    Running,
    Suspended,
    Completed,
    Cancelled,
    Panicked,
}

impl TaskState {
    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Panicked)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskPollCompletion {
    Ready,
    Pending,
    Completed,
    Cancelled,
    Panicked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationObservation {
    Active,
    Requested,
    Masked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskLifecycleError {
    AlreadyStarted(TaskId),
    NotStarted(TaskId),
    AlreadyRunning(TaskId),
    NotRunning(TaskId),
    Terminal(TaskId),
    AdmissionAlreadyPolled(TaskId),
    OwnerMismatch(TaskId),
    CancellationTokenNotBound(TaskId),
    CancellationTokenAlreadyBound(TaskId),
    CancellationTokenMismatch {
        expected: CancellationTokenId,
        found: CancellationTokenId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskGroupExit {
    BodyCompleted,
    BodyFailed,
    Cancelled,
    BodyPanicked,
    ChildPanicked(TaskId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskGroupState {
    Open,
    Closing(TaskGroupExit),
    Closed(TaskGroupExit),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskGroupLifecycleError {
    NotOpen(TaskGroupId),
    NotClosing(TaskGroupId),
    DuplicateChild(TaskId),
    UnknownChild(TaskId),
    ChildOwnerMismatch(TaskId),
    ChildNotTerminal(TaskId),
    ChildrenRemain(TaskGroupId),
    Task(TaskLifecycleError),
}

/// Closed semantic state for one task control record.
///
/// The record contains no callable pointer, frame value, scheduler queue, or
/// backend object. Native and interpreter runtimes use it while retaining
/// their own typed frame storage and completion representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskLifecycle {
    id: TaskId,
    state: TaskState,
    owner: Option<TaskOwner>,
    cancellation_token: Option<CancellationTokenId>,
    cancellation_requested: bool,
}

impl TaskLifecycle {
    #[must_use]
    pub const fn created(id: TaskId) -> Self {
        Self {
            id,
            state: TaskState::Created,
            owner: None,
            cancellation_token: None,
            cancellation_requested: false,
        }
    }

    #[must_use]
    pub const fn id(self) -> TaskId {
        self.id
    }

    #[must_use]
    pub const fn state(self) -> TaskState {
        self.state
    }

    #[must_use]
    pub const fn owner(self) -> Option<TaskOwner> {
        self.owner
    }

    #[must_use]
    pub const fn cancellation_token(self) -> Option<CancellationTokenId> {
        self.cancellation_token
    }

    #[must_use]
    pub const fn completed(self) -> bool {
        matches!(self.state, TaskState::Completed)
    }

    /// Transfers a cold task to its sole direct-await or group owner.
    ///
    /// # Errors
    ///
    /// Rejects every repeated start without changing the original owner.
    pub fn start(&mut self, owner: TaskOwner) -> Result<(), TaskLifecycleError> {
        if self.state != TaskState::Created || self.owner.is_some() {
            return Err(TaskLifecycleError::AlreadyStarted(self.id));
        }
        self.owner = Some(owner);
        self.state = TaskState::Ready;
        Ok(())
    }

    /// Restores a provisionally owned task after scheduler admission rejects
    /// it before the first poll.
    ///
    /// # Errors
    ///
    /// Rejects a task whose exact provisional owner does not match or whose
    /// first poll has already begun.
    pub fn rollback_unpolled_start(
        &mut self,
        owner: TaskOwner,
        unbind_cancellation_token: bool,
    ) -> Result<(), TaskLifecycleError> {
        if self.owner != Some(owner) {
            return Err(TaskLifecycleError::OwnerMismatch(self.id));
        }
        if self.state != TaskState::Ready {
            return Err(TaskLifecycleError::AdmissionAlreadyPolled(self.id));
        }
        self.state = TaskState::Created;
        self.owner = None;
        if unbind_cancellation_token {
            self.cancellation_token = None;
        }
        Ok(())
    }

    /// Marks a ready or suspended coroutine as running for one bounded poll.
    ///
    /// # Errors
    ///
    /// Rejects cold, already-running, and terminal tasks.
    pub fn begin_poll(&mut self) -> Result<(), TaskLifecycleError> {
        match self.state {
            TaskState::Ready | TaskState::Suspended => {
                self.state = TaskState::Running;
                Ok(())
            }
            TaskState::Created => Err(TaskLifecycleError::NotStarted(self.id)),
            TaskState::Running => Err(TaskLifecycleError::AlreadyRunning(self.id)),
            TaskState::Completed | TaskState::Cancelled | TaskState::Panicked => {
                Err(TaskLifecycleError::Terminal(self.id))
            }
        }
    }

    /// Commits one coroutine poll result.
    ///
    /// # Errors
    ///
    /// Rejects a result unless this exact task is currently running.
    pub fn finish_poll(
        &mut self,
        completion: TaskPollCompletion,
    ) -> Result<(), TaskLifecycleError> {
        if self.state != TaskState::Running {
            return if self.state.terminal() {
                Err(TaskLifecycleError::Terminal(self.id))
            } else {
                Err(TaskLifecycleError::NotRunning(self.id))
            };
        }
        self.state = match completion {
            TaskPollCompletion::Ready => TaskState::Ready,
            TaskPollCompletion::Pending => TaskState::Suspended,
            TaskPollCompletion::Completed => TaskState::Completed,
            TaskPollCompletion::Cancelled => TaskState::Cancelled,
            TaskPollCompletion::Panicked => TaskState::Panicked,
        };
        Ok(())
    }

    /// Binds the exact explicit cancellation token captured by this task.
    ///
    /// # Errors
    ///
    /// Rejects a second token even when its numeric identity is equal.
    pub fn bind_cancellation_token(
        &mut self,
        token: CancellationTokenId,
    ) -> Result<(), TaskLifecycleError> {
        if self.cancellation_token.is_some() {
            return Err(TaskLifecycleError::CancellationTokenAlreadyBound(self.id));
        }
        self.cancellation_token = Some(token);
        Ok(())
    }

    /// Requests cooperative cancellation through the task's exact token.
    ///
    /// Returns whether this call changed the pending request.
    ///
    /// # Errors
    ///
    /// Rejects an unbound or different token and every terminal task.
    pub fn request_cancellation(
        &mut self,
        token: CancellationTokenId,
    ) -> Result<bool, TaskLifecycleError> {
        if self.state.terminal() {
            return Err(TaskLifecycleError::Terminal(self.id));
        }
        let Some(expected) = self.cancellation_token else {
            return Err(TaskLifecycleError::CancellationTokenNotBound(self.id));
        };
        if expected != token {
            return Err(TaskLifecycleError::CancellationTokenMismatch {
                expected,
                found: token,
            });
        }
        let changed = !self.cancellation_requested;
        self.cancellation_requested = true;
        Ok(changed)
    }

    #[must_use]
    pub const fn cancellation_observation(self, masked: bool) -> CancellationObservation {
        if !self.cancellation_requested {
            CancellationObservation::Active
        } else if masked {
            CancellationObservation::Masked
        } else {
            CancellationObservation::Requested
        }
    }
}

/// Lexical ownership and join state for one structured task group.
///
/// Child execution and completion values remain in their typed task records.
/// This contract retains only identities, the explicit group token, and the
/// closed exit whose propagation is delayed until every child has joined.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskGroupLifecycle {
    id: TaskGroupId,
    cancellation_token: CancellationTokenId,
    state: TaskGroupState,
    unfinished_children: BTreeSet<TaskId>,
    child_panic: Option<TaskId>,
}

impl TaskGroupLifecycle {
    #[must_use]
    pub const fn open(id: TaskGroupId, cancellation_token: CancellationTokenId) -> Self {
        Self {
            id,
            cancellation_token,
            state: TaskGroupState::Open,
            unfinished_children: BTreeSet::new(),
            child_panic: None,
        }
    }

    #[must_use]
    pub const fn id(&self) -> TaskGroupId {
        self.id
    }

    #[must_use]
    pub const fn cancellation_token(&self) -> CancellationTokenId {
        self.cancellation_token
    }

    #[must_use]
    pub const fn state(&self) -> TaskGroupState {
        self.state
    }

    #[must_use]
    pub fn unfinished_children(&self) -> Vec<TaskId> {
        self.unfinished_children.iter().copied().collect()
    }

    /// Transfers one cold task to this open group and binds its exact token.
    ///
    /// # Errors
    ///
    /// Rejects a closing group, duplicate identity, non-cold task, or a task
    /// already bound to a different token before mutating either owner.
    pub fn start_child(
        &mut self,
        child: &mut TaskLifecycle,
    ) -> Result<(), TaskGroupLifecycleError> {
        if self.state != TaskGroupState::Open {
            return Err(TaskGroupLifecycleError::NotOpen(self.id));
        }
        if self.unfinished_children.contains(&child.id()) {
            return Err(TaskGroupLifecycleError::DuplicateChild(child.id()));
        }
        if let Some(token) = child.cancellation_token()
            && token != self.cancellation_token
        {
            return Err(TaskGroupLifecycleError::Task(
                TaskLifecycleError::CancellationTokenMismatch {
                    expected: token,
                    found: self.cancellation_token,
                },
            ));
        }
        child
            .start(TaskOwner::Group(self.id))
            .map_err(TaskGroupLifecycleError::Task)?;
        if child.cancellation_token().is_none() {
            child
                .bind_cancellation_token(self.cancellation_token)
                .map_err(TaskGroupLifecycleError::Task)?;
        }
        let inserted = self.unfinished_children.insert(child.id());
        debug_assert!(inserted);
        Ok(())
    }

    /// Rolls back one child whose scheduler admission failed before polling.
    ///
    /// # Errors
    ///
    /// Rejects a non-open group, an unknown child, or any child whose exact
    /// provisional ownership can no longer be restored safely.
    pub fn rollback_unpolled_child(
        &mut self,
        child: &mut TaskLifecycle,
        unbind_cancellation_token: bool,
    ) -> Result<(), TaskGroupLifecycleError> {
        if self.state != TaskGroupState::Open {
            return Err(TaskGroupLifecycleError::NotOpen(self.id));
        }
        if !self.unfinished_children.contains(&child.id()) {
            return Err(TaskGroupLifecycleError::UnknownChild(child.id()));
        }
        child
            .rollback_unpolled_start(TaskOwner::Group(self.id), unbind_cancellation_token)
            .map_err(TaskGroupLifecycleError::Task)?;
        self.unfinished_children.remove(&child.id());
        Ok(())
    }

    /// Records one terminal child as joined by this exact owner.
    ///
    /// # Errors
    ///
    /// Rejects foreign, unknown, and nonterminal tasks.
    pub fn join_child(&mut self, child: &TaskLifecycle) -> Result<(), TaskGroupLifecycleError> {
        if child.owner() != Some(TaskOwner::Group(self.id)) {
            return Err(TaskGroupLifecycleError::ChildOwnerMismatch(child.id()));
        }
        if !self.unfinished_children.contains(&child.id()) {
            return Err(TaskGroupLifecycleError::UnknownChild(child.id()));
        }
        if !child.state().terminal() {
            return Err(TaskGroupLifecycleError::ChildNotTerminal(child.id()));
        }
        if child.state() == TaskState::Panicked {
            let panic = self
                .child_panic
                .map_or(child.id(), |existing| existing.min(child.id()));
            self.child_panic = Some(panic);
            if matches!(
                self.state,
                TaskGroupState::Closing(
                    TaskGroupExit::BodyCompleted
                        | TaskGroupExit::BodyFailed
                        | TaskGroupExit::Cancelled
                )
            ) {
                self.state = TaskGroupState::Closing(TaskGroupExit::ChildPanicked(panic));
            }
        }
        self.unfinished_children.remove(&child.id());
        Ok(())
    }

    /// Starts lexical group closure and returns unfinished children in stable
    /// identity order for explicit cancellation requests and joining.
    ///
    /// # Errors
    ///
    /// Rejects repeated closure so the original exit cannot be replaced.
    pub fn begin_close(
        &mut self,
        exit: TaskGroupExit,
    ) -> Result<Vec<TaskId>, TaskGroupLifecycleError> {
        if self.state != TaskGroupState::Open {
            return Err(TaskGroupLifecycleError::NotOpen(self.id));
        }
        let exit = match (exit, self.child_panic) {
            (TaskGroupExit::BodyPanicked, _) => TaskGroupExit::BodyPanicked,
            (exit, None) => exit,
            (_, Some(child)) => TaskGroupExit::ChildPanicked(child),
        };
        self.state = TaskGroupState::Closing(exit);
        Ok(self.unfinished_children())
    }

    /// Closes the group and releases its retained exit only after every child
    /// has reached a terminal state and joined.
    ///
    /// # Errors
    ///
    /// Rejects open/already-closed groups and groups with live children.
    pub fn complete_close(&mut self) -> Result<TaskGroupExit, TaskGroupLifecycleError> {
        let TaskGroupState::Closing(exit) = self.state else {
            return Err(TaskGroupLifecycleError::NotClosing(self.id));
        };
        if !self.unfinished_children.is_empty() {
            return Err(TaskGroupLifecycleError::ChildrenRemain(self.id));
        }
        self.state = TaskGroupState::Closed(exit);
        Ok(exit)
    }
}
