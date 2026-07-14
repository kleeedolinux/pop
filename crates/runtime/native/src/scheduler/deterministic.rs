//! Single-threaded enabled-set record/replay scheduler.

use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};

use pop_runtime_collector::SchedulerId;

use super::{
    SchedulerConfiguration, SchedulerDecision, SchedulerError, SchedulerTask, SchedulerTaskContext,
    SchedulerTaskId, SchedulerTaskMobility, SchedulerTaskPoll, SchedulerTaskState,
    SchedulerTelemetry, SchedulerWorkerId,
};

enum ReplayMode {
    Recording,
    Replaying {
        expected: Vec<SchedulerDecision>,
        cursor: usize,
    },
}

struct DeterministicTask {
    task: Box<dyn SchedulerTask>,
    state: SchedulerTaskState,
    scheduler: SchedulerId,
    mobility: SchedulerTaskMobility,
    cancellation_requested: bool,
}

pub struct DeterministicScheduler {
    configuration: SchedulerConfiguration,
    tasks: BTreeMap<SchedulerTaskId, DeterministicTask>,
    ready: VecDeque<SchedulerTaskId>,
    next_task: u64,
    next_scheduler: usize,
    mode: ReplayMode,
    transcript: Vec<SchedulerDecision>,
    telemetry: SchedulerTelemetry,
}

impl DeterministicScheduler {
    #[must_use]
    pub fn recording(configuration: SchedulerConfiguration) -> Self {
        Self::new(configuration, ReplayMode::Recording)
    }

    #[must_use]
    pub fn replaying(
        configuration: SchedulerConfiguration,
        expected: Vec<SchedulerDecision>,
    ) -> Self {
        Self::new(
            configuration,
            ReplayMode::Replaying {
                expected,
                cursor: 0,
            },
        )
    }

    fn new(configuration: SchedulerConfiguration, mode: ReplayMode) -> Self {
        Self {
            configuration,
            tasks: BTreeMap::new(),
            ready: VecDeque::new(),
            next_task: 1,
            next_scheduler: 0,
            mode,
            transcript: Vec::new(),
            telemetry: SchedulerTelemetry::default(),
        }
    }

    /// Adds one already-owned task to the deterministic injection queue.
    ///
    /// # Errors
    ///
    /// Rejects retained-task, injection-queue, or identity exhaustion.
    pub fn schedule<T: SchedulerTask>(
        &mut self,
        task: T,
    ) -> Result<SchedulerTaskId, SchedulerError> {
        let scheduler = self.next_scheduler_id();
        self.schedule_on(scheduler, SchedulerTaskMobility::Movable, task)
    }

    /// Adds one already-owned task to an exact virtual scheduler.
    ///
    /// # Errors
    ///
    /// Rejects unknown schedulers or bounded-capacity exhaustion.
    pub fn schedule_on<T: SchedulerTask>(
        &mut self,
        scheduler: SchedulerId,
        mobility: SchedulerTaskMobility,
        task: T,
    ) -> Result<SchedulerTaskId, SchedulerError> {
        self.validate_scheduler(scheduler)?;
        if self.tasks.len() >= self.configuration.task_capacity {
            return Err(SchedulerError::TaskCapacity);
        }
        if self.ready.len() >= self.configuration.injection_queue_capacity {
            return Err(SchedulerError::InjectionQueueCapacity);
        }
        let id = SchedulerTaskId::new(self.next_task);
        self.next_task = self
            .next_task
            .checked_add(1)
            .ok_or(SchedulerError::IdentityOverflow)?;
        self.tasks.insert(
            id,
            DeterministicTask {
                task: Box::new(task),
                state: SchedulerTaskState::Ready,
                scheduler,
                mobility,
                cancellation_requested: false,
            },
        );
        self.ready.push_back(id);
        self.telemetry.tasks_scheduled = self.telemetry.tasks_scheduled.saturating_add(1);
        self.refresh_counts();
        Ok(id)
    }

    /// Runs ready tasks until no task is enabled or the poll bound is reached.
    ///
    /// # Errors
    ///
    /// Fails closed on replay mismatch/exhaustion or poll-budget exhaustion.
    pub fn run_until_idle(&mut self, maximum_polls: usize) -> Result<(), SchedulerError> {
        let mut polls = 0;
        while !self.ready.is_empty() {
            if polls >= maximum_polls {
                return Err(SchedulerError::PollBudgetExhausted);
            }
            let enabled = self.ready.iter().copied().collect::<Vec<_>>();
            let (selected, scheduler) = self.select(&enabled)?;
            let position = self
                .ready
                .iter()
                .position(|candidate| *candidate == selected)
                .ok_or(SchedulerError::ReplayEnabledSetMismatch)?;
            self.ready.remove(position);
            self.record_decision(enabled, selected, scheduler)?;
            self.poll_task(selected)?;
            polls += 1;
        }
        if let ReplayMode::Replaying { expected, cursor } = &self.mode
            && *cursor != expected.len()
        {
            return Err(SchedulerError::ReplayEnabledSetMismatch);
        }
        self.refresh_counts();
        Ok(())
    }

    /// Marks a suspended/running task ready exactly once.
    ///
    /// # Errors
    ///
    /// Rejects unknown tasks or a full ready queue.
    pub fn wake(&mut self, id: SchedulerTaskId) -> Result<bool, SchedulerError> {
        let record = self
            .tasks
            .get_mut(&id)
            .ok_or(SchedulerError::UnknownTask(id))?;
        self.telemetry.wake_requests = self.telemetry.wake_requests.saturating_add(1);
        match record.state {
            SchedulerTaskState::Suspended => {
                if self.ready.len() >= self.configuration.local_queue_capacity {
                    return Err(SchedulerError::LocalQueueCapacity);
                }
                record.state = SchedulerTaskState::Ready;
                self.ready.push_back(id);
                self.refresh_counts();
                Ok(true)
            }
            SchedulerTaskState::Ready | SchedulerTaskState::Running => {
                self.telemetry.coalesced_wakes = self.telemetry.coalesced_wakes.saturating_add(1);
                Ok(false)
            }
            SchedulerTaskState::Completed
            | SchedulerTaskState::Cancelled
            | SchedulerTaskState::Panicked => Ok(false),
        }
    }

    /// Requests cooperative cancellation and enables a suspended task.
    ///
    /// # Errors
    ///
    /// Rejects unknown tasks or a full ready queue.
    pub fn request_cancellation(&mut self, id: SchedulerTaskId) -> Result<bool, SchedulerError> {
        let record = self
            .tasks
            .get_mut(&id)
            .ok_or(SchedulerError::UnknownTask(id))?;
        if record.state.terminal() || record.cancellation_requested {
            return Ok(false);
        }
        record.cancellation_requested = true;
        self.telemetry.cancellations_requested =
            self.telemetry.cancellations_requested.saturating_add(1);
        if record.state == SchedulerTaskState::Suspended {
            if self.ready.len() >= self.configuration.local_queue_capacity {
                record.cancellation_requested = false;
                self.telemetry.cancellations_requested =
                    self.telemetry.cancellations_requested.saturating_sub(1);
                return Err(SchedulerError::LocalQueueCapacity);
            }
            record.state = SchedulerTaskState::Ready;
            self.ready.push_back(id);
        }
        self.refresh_counts();
        Ok(true)
    }

    #[must_use]
    pub fn transcript(&self) -> &[SchedulerDecision] {
        &self.transcript
    }

    #[must_use]
    pub fn replay_complete(&self) -> bool {
        match &self.mode {
            ReplayMode::Recording => true,
            ReplayMode::Replaying { expected, cursor } => *cursor == expected.len(),
        }
    }

    /// Returns the exact retained state for one task.
    ///
    /// # Errors
    ///
    /// Rejects an unknown task identity.
    pub fn task_state(&self, id: SchedulerTaskId) -> Result<SchedulerTaskState, SchedulerError> {
        self.tasks
            .get(&id)
            .map(|record| record.state)
            .ok_or(SchedulerError::UnknownTask(id))
    }

    /// Releases scheduler metadata only after a task reaches terminal state.
    ///
    /// # Errors
    ///
    /// Rejects unknown or nonterminal tasks.
    pub fn release_terminal_task(&mut self, id: SchedulerTaskId) -> Result<(), SchedulerError> {
        let record = self.tasks.get(&id).ok_or(SchedulerError::UnknownTask(id))?;
        if !record.state.terminal() {
            return Err(SchedulerError::TaskNotTerminal(id));
        }
        self.tasks.remove(&id);
        self.refresh_counts();
        Ok(())
    }

    #[must_use]
    pub fn telemetry(&self) -> SchedulerTelemetry {
        let mut telemetry = self.telemetry;
        telemetry.worker_threads_used = self
            .tasks
            .values()
            .filter(|record| {
                record.state != SchedulerTaskState::Suspended
                    || record.mobility == SchedulerTaskMobility::Movable
            })
            .map(|record| record.scheduler)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        telemetry
    }

    fn next_scheduler_id(&mut self) -> SchedulerId {
        let index = self.next_scheduler;
        self.next_scheduler = (self.next_scheduler + 1) % self.configuration.scheduler_count;
        SchedulerId::new(u32::try_from(index + 1).expect("validated scheduler identity range"))
    }

    fn validate_scheduler(&self, scheduler: SchedulerId) -> Result<(), SchedulerError> {
        let raw = scheduler.raw();
        if raw == 0 || raw as usize > self.configuration.scheduler_count {
            Err(SchedulerError::UnknownScheduler(scheduler))
        } else {
            Ok(())
        }
    }

    fn select(
        &self,
        enabled: &[SchedulerTaskId],
    ) -> Result<(SchedulerTaskId, SchedulerId), SchedulerError> {
        match &self.mode {
            ReplayMode::Recording => {
                let selected = *enabled.first().ok_or(SchedulerError::ReplayExhausted)?;
                let scheduler = self
                    .tasks
                    .get(&selected)
                    .map(|record| record.scheduler)
                    .ok_or(SchedulerError::UnknownTask(selected))?;
                Ok((selected, scheduler))
            }
            ReplayMode::Replaying { expected, cursor } => {
                let decision = expected
                    .get(*cursor)
                    .ok_or(SchedulerError::ReplayExhausted)?;
                if decision.enabled() != enabled {
                    return Err(SchedulerError::ReplayEnabledSetMismatch);
                }
                Ok((decision.selected(), decision.scheduler()))
            }
        }
    }

    fn record_decision(
        &mut self,
        enabled: Vec<SchedulerTaskId>,
        selected: SchedulerTaskId,
        scheduler: SchedulerId,
    ) -> Result<(), SchedulerError> {
        let decision_number = u64::try_from(self.transcript.len())
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(SchedulerError::IdentityOverflow)?;
        if let ReplayMode::Replaying { expected, cursor } = &mut self.mode {
            let expected_decision = expected
                .get(*cursor)
                .ok_or(SchedulerError::ReplayExhausted)?;
            if expected_decision.selected() != selected
                || expected_decision.scheduler() != scheduler
                || expected_decision.decision() != decision_number
            {
                return Err(SchedulerError::ReplayEnabledSetMismatch);
            }
            *cursor += 1;
        }
        self.transcript.push(SchedulerDecision::new(
            decision_number,
            enabled,
            selected,
            scheduler,
        ));
        Ok(())
    }

    fn poll_task(&mut self, id: SchedulerTaskId) -> Result<(), SchedulerError> {
        let record = self
            .tasks
            .get_mut(&id)
            .ok_or(SchedulerError::UnknownTask(id))?;
        record.state = SchedulerTaskState::Running;
        let context = SchedulerTaskContext::new(
            id,
            record.scheduler,
            SchedulerWorkerId::new(record.scheduler.raw()),
            record.cancellation_requested,
        );
        self.telemetry.polls = self.telemetry.polls.saturating_add(1);
        let result = catch_unwind(AssertUnwindSafe(|| record.task.poll(&context)));
        match result {
            Ok(SchedulerTaskPoll::Ready) => {
                record.state = SchedulerTaskState::Ready;
                self.ready.push_back(id);
            }
            Ok(SchedulerTaskPoll::Pending) => {
                record.state = SchedulerTaskState::Suspended;
                self.telemetry.suspensions = self.telemetry.suspensions.saturating_add(1);
            }
            Ok(SchedulerTaskPoll::Complete) => {
                record.state = SchedulerTaskState::Completed;
                self.telemetry.completions = self.telemetry.completions.saturating_add(1);
            }
            Ok(SchedulerTaskPoll::Cancelled) => {
                record.state = SchedulerTaskState::Cancelled;
                self.telemetry.cancellations_observed =
                    self.telemetry.cancellations_observed.saturating_add(1);
            }
            Err(_) => {
                record.state = SchedulerTaskState::Panicked;
                self.telemetry.panics = self.telemetry.panics.saturating_add(1);
            }
        }
        self.refresh_counts();
        Ok(())
    }

    fn refresh_counts(&mut self) {
        self.telemetry.retained_tasks = self.tasks.len();
        self.telemetry.ready_tasks = 0;
        self.telemetry.running_tasks = 0;
        self.telemetry.suspended_tasks = 0;
        self.telemetry.terminal_tasks = 0;
        for record in self.tasks.values() {
            match record.state {
                SchedulerTaskState::Ready => self.telemetry.ready_tasks += 1,
                SchedulerTaskState::Running => self.telemetry.running_tasks += 1,
                SchedulerTaskState::Suspended => self.telemetry.suspended_tasks += 1,
                SchedulerTaskState::Completed
                | SchedulerTaskState::Cancelled
                | SchedulerTaskState::Panicked => self.telemetry.terminal_tasks += 1,
            }
        }
    }
}
