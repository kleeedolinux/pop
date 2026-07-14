//! Per-worker bounded queues and deterministic result collection.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use pop_runtime_interface::{ManagedReference, RuntimeFailure};

use crate::heap::{Allocation, SlotValue};
use crate::relocation::RelocationAllocation;

use super::super::heap::LargeObjectScanChunk;
use super::model::{BackgroundWorkerConfig, BackgroundWorkerStartError, BackgroundWorkerTelemetry};

pub(crate) struct MarkTask {
    pub(crate) reference: ManagedReference,
    pub(crate) slots: Vec<SlotValue>,
    pub(crate) large_object_scan_chunk: Option<LargeObjectScanChunk>,
}

pub(crate) struct CardRefinementTask {
    pub(crate) owner: ManagedReference,
    pub(crate) allocation: Allocation,
}

pub(crate) struct EvacuationRewriteTask {
    pub(crate) destination: ManagedReference,
    pub(crate) allocation: RelocationAllocation,
}

enum WorkerCommand {
    Mark {
        sequence: u64,
        task: MarkTask,
    },
    RefineCard {
        sequence: u64,
        task: CardRefinementTask,
        young: Arc<BTreeSet<ManagedReference>>,
    },
    Sweep {
        sequence: u64,
        reference: ManagedReference,
    },
    Evacuate {
        sequence: u64,
        task: EvacuationRewriteTask,
        relocations: Arc<BTreeMap<ManagedReference, ManagedReference>>,
    },
}

struct QueuedWorkerCommand {
    command: WorkerCommand,
    stolen: bool,
}

struct WorkerQueue {
    commands: Mutex<VecDeque<WorkerCommand>>,
    space_available: Condvar,
}

struct SharedWorkerQueues {
    queues: Vec<WorkerQueue>,
    idle_gate: Mutex<()>,
    work_available: Condvar,
    shutdown: AtomicBool,
    queue_capacity: usize,
}

enum WorkerOutcome {
    Mark {
        reference: ManagedReference,
        large_object_scan_chunk: Option<LargeObjectScanChunk>,
        children: Result<Vec<ManagedReference>, ()>,
    },
    RefinedCard {
        owner: ManagedReference,
        children: Result<Vec<ManagedReference>, ()>,
    },
    Sweep(ManagedReference),
    Evacuated {
        destination: ManagedReference,
        allocation: RelocationAllocation,
        fields_updated: usize,
    },
}

struct WorkerResult {
    sequence: u64,
    worker: usize,
    stolen: bool,
    outcome: WorkerOutcome,
}

pub(crate) struct MarkResult {
    pub(crate) reference: ManagedReference,
    pub(crate) large_object_scan_chunk: Option<LargeObjectScanChunk>,
    pub(crate) children: Vec<ManagedReference>,
}

pub(crate) struct EvacuationRewriteResult {
    pub(crate) destination: ManagedReference,
    pub(crate) allocation: RelocationAllocation,
    pub(crate) fields_updated: usize,
}

pub(crate) struct BackgroundWorkerPool {
    queues: Arc<SharedWorkerQueues>,
    results: Receiver<WorkerResult>,
    threads: Vec<JoinHandle<()>>,
    next_worker: usize,
    next_sequence: u64,
    workers_used: BTreeSet<usize>,
    telemetry: BackgroundWorkerTelemetry,
}

impl SharedWorkerQueues {
    fn new(worker_count: usize, queue_capacity: usize) -> Self {
        Self {
            queues: (0..worker_count)
                .map(|_| WorkerQueue {
                    commands: Mutex::new(VecDeque::new()),
                    space_available: Condvar::new(),
                })
                .collect(),
            idle_gate: Mutex::new(()),
            work_available: Condvar::new(),
            shutdown: AtomicBool::new(false),
            queue_capacity,
        }
    }

    fn submit(&self, worker: usize, command: WorkerCommand) -> Result<(), RuntimeFailure> {
        let queue = self
            .queues
            .get(worker)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let mut commands = queue
            .commands
            .lock()
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        while !self.shutdown.load(Ordering::Acquire) && commands.len() >= self.queue_capacity {
            commands = queue
                .space_available
                .wait(commands)
                .map_err(|_| RuntimeFailure::runtime_invariant())?;
        }
        if self.shutdown.load(Ordering::Acquire) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        commands.push_back(command);
        drop(commands);
        let _idle = self
            .idle_gate
            .lock()
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        self.work_available.notify_one();
        Ok(())
    }

    fn take(&self, worker: usize) -> Option<QueuedWorkerCommand> {
        loop {
            if let Some(command) = self.try_take(worker).ok()? {
                return Some(command);
            }
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            let idle = self.idle_gate.lock().ok()?;
            if let Some(command) = self.try_take(worker).ok()? {
                return Some(command);
            }
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            drop(self.work_available.wait(idle).ok()?);
        }
    }

    fn try_take(&self, worker: usize) -> Result<Option<QueuedWorkerCommand>, ()> {
        let Some(local) = self.queues.get(worker) else {
            return Err(());
        };
        if let Some(command) = local.commands.lock().map_err(|_| ())?.pop_front() {
            local.space_available.notify_one();
            return Ok(Some(QueuedWorkerCommand {
                command,
                stolen: false,
            }));
        }
        for offset in 1..self.queues.len() {
            let victim = (worker + offset) % self.queues.len();
            let queue = &self.queues[victim];
            if let Some(command) = queue.commands.lock().map_err(|_| ())?.pop_back() {
                queue.space_available.notify_one();
                return Ok(Some(QueuedWorkerCommand {
                    command,
                    stolen: true,
                }));
            }
        }
        Ok(None)
    }

    const fn worker_count(&self) -> usize {
        self.queues.len()
    }

    fn shutdown(&self) {
        if let Ok(_idle) = self.idle_gate.lock() {
            self.shutdown.store(true, Ordering::Release);
            for queue in &self.queues {
                if let Ok(_commands) = queue.commands.lock() {
                    queue.space_available.notify_all();
                }
            }
            self.work_available.notify_all();
        }
    }
}

impl BackgroundWorkerPool {
    pub(crate) fn new(config: BackgroundWorkerConfig) -> Result<Self, BackgroundWorkerStartError> {
        let (result_sender, results) = mpsc::channel();
        let queues = Arc::new(SharedWorkerQueues::new(
            config.worker_count,
            config.queue_capacity,
        ));
        let mut threads: Vec<JoinHandle<()>> = Vec::with_capacity(config.worker_count);
        for worker in 0..config.worker_count {
            let worker_queues = Arc::clone(&queues);
            let worker_results = result_sender.clone();
            let Ok(handle) = thread::Builder::new()
                .name(format!("pop-gc-{worker}"))
                .spawn(move || worker_loop(worker, &worker_queues, &worker_results))
            else {
                queues.shutdown();
                for thread in threads {
                    let _ = thread.join();
                }
                return Err(BackgroundWorkerStartError::ThreadSpawn);
            };
            threads.push(handle);
        }
        drop(result_sender);
        Ok(Self {
            queues,
            results,
            threads,
            next_worker: 0,
            next_sequence: 1,
            workers_used: BTreeSet::new(),
            telemetry: BackgroundWorkerTelemetry {
                workers_started: config.worker_count,
                ..BackgroundWorkerTelemetry::default()
            },
        })
    }

    pub(crate) fn mark(&mut self, tasks: Vec<MarkTask>) -> Result<Vec<MarkResult>, RuntimeFailure> {
        let count = tasks.len();
        for task in tasks {
            let sequence = self.next_sequence()?;
            self.submit(WorkerCommand::Mark { sequence, task })?;
        }
        let results = self.collect(count)?;
        let mut marked = Vec::with_capacity(count);
        for result in results {
            match result.outcome {
                WorkerOutcome::Mark {
                    reference,
                    large_object_scan_chunk,
                    children,
                } => {
                    marked.push(MarkResult {
                        reference,
                        large_object_scan_chunk,
                        children: children.map_err(|()| RuntimeFailure::runtime_invariant())?,
                    });
                    self.telemetry.mark_jobs_completed =
                        self.telemetry.mark_jobs_completed.saturating_add(1);
                }
                WorkerOutcome::RefinedCard { .. }
                | WorkerOutcome::Sweep(_)
                | WorkerOutcome::Evacuated { .. } => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            }
        }
        self.complete_batch(count);
        Ok(marked)
    }

    pub(crate) fn refine_cards(
        &mut self,
        tasks: Vec<CardRefinementTask>,
        young: &Arc<BTreeSet<ManagedReference>>,
    ) -> Result<BTreeMap<ManagedReference, Vec<ManagedReference>>, RuntimeFailure> {
        let count = tasks.len();
        for task in tasks {
            let sequence = self.next_sequence()?;
            self.submit(WorkerCommand::RefineCard {
                sequence,
                task,
                young: Arc::clone(young),
            })?;
        }
        let results = self.collect(count)?;
        let mut refined = BTreeMap::new();
        for result in results {
            match result.outcome {
                WorkerOutcome::RefinedCard { owner, children } => {
                    if refined
                        .insert(
                            owner,
                            children.map_err(|()| RuntimeFailure::runtime_invariant())?,
                        )
                        .is_some()
                    {
                        return Err(RuntimeFailure::runtime_invariant());
                    }
                    self.telemetry.card_refinement_jobs_completed = self
                        .telemetry
                        .card_refinement_jobs_completed
                        .saturating_add(1);
                }
                WorkerOutcome::Mark { .. }
                | WorkerOutcome::Sweep(_)
                | WorkerOutcome::Evacuated { .. } => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            }
        }
        self.complete_batch(count);
        Ok(refined)
    }

    pub(crate) fn sweep(
        &mut self,
        references: Vec<ManagedReference>,
    ) -> Result<Vec<ManagedReference>, RuntimeFailure> {
        let count = references.len();
        for reference in references {
            let sequence = self.next_sequence()?;
            self.submit(WorkerCommand::Sweep {
                sequence,
                reference,
            })?;
        }
        let results = self.collect(count)?;
        let mut swept = Vec::with_capacity(count);
        for result in results {
            match result.outcome {
                WorkerOutcome::Sweep(reference) => {
                    swept.push(reference);
                    self.telemetry.sweep_jobs_completed =
                        self.telemetry.sweep_jobs_completed.saturating_add(1);
                }
                WorkerOutcome::Mark { .. }
                | WorkerOutcome::RefinedCard { .. }
                | WorkerOutcome::Evacuated { .. } => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            }
        }
        self.complete_batch(count);
        Ok(swept)
    }

    pub(crate) fn evacuate(
        &mut self,
        tasks: Vec<EvacuationRewriteTask>,
        relocations: &Arc<BTreeMap<ManagedReference, ManagedReference>>,
    ) -> Result<Vec<EvacuationRewriteResult>, RuntimeFailure> {
        let count = tasks.len();
        for task in tasks {
            let sequence = self.next_sequence()?;
            self.submit(WorkerCommand::Evacuate {
                sequence,
                task,
                relocations: Arc::clone(relocations),
            })?;
        }
        let results = self.collect(count)?;
        let mut evacuated = Vec::with_capacity(count);
        for result in results {
            match result.outcome {
                WorkerOutcome::Evacuated {
                    destination,
                    allocation,
                    fields_updated,
                } => {
                    evacuated.push(EvacuationRewriteResult {
                        destination,
                        allocation,
                        fields_updated,
                    });
                    self.telemetry.evacuation_jobs_completed =
                        self.telemetry.evacuation_jobs_completed.saturating_add(1);
                }
                WorkerOutcome::Mark { .. }
                | WorkerOutcome::RefinedCard { .. }
                | WorkerOutcome::Sweep(_) => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            }
        }
        self.complete_batch(count);
        Ok(evacuated)
    }

    pub(crate) fn telemetry(&self) -> BackgroundWorkerTelemetry {
        BackgroundWorkerTelemetry {
            worker_threads_used: self.workers_used.len(),
            ..self.telemetry
        }
    }

    fn next_sequence(&mut self) -> Result<u64, RuntimeFailure> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        Ok(sequence)
    }

    fn submit(&mut self, command: WorkerCommand) -> Result<(), RuntimeFailure> {
        let worker = self.next_worker;
        self.next_worker = (self.next_worker + 1) % self.queues.worker_count();
        self.queues.submit(worker, command)?;
        self.telemetry.jobs_submitted = self.telemetry.jobs_submitted.saturating_add(1);
        Ok(())
    }

    fn collect(&mut self, count: usize) -> Result<Vec<WorkerResult>, RuntimeFailure> {
        let mut completed = Vec::with_capacity(count);
        for _ in 0..count {
            let result = self
                .results
                .recv()
                .map_err(|_| RuntimeFailure::runtime_invariant())?;
            self.workers_used.insert(result.worker);
            if result.stolen {
                self.telemetry.jobs_stolen = self.telemetry.jobs_stolen.saturating_add(1);
            }
            completed.push(result);
        }
        completed.sort_by_key(|result| result.sequence);
        self.telemetry.jobs_completed = self
            .telemetry
            .jobs_completed
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        Ok(completed)
    }

    fn complete_batch(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.telemetry.batches_completed = self.telemetry.batches_completed.saturating_add(1);
        self.telemetry.maximum_batch_size = self.telemetry.maximum_batch_size.max(count);
    }
}

impl Drop for BackgroundWorkerPool {
    fn drop(&mut self) {
        self.queues.shutdown();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn worker_loop(worker: usize, queues: &SharedWorkerQueues, results: &mpsc::Sender<WorkerResult>) {
    while let Some(queued) = queues.take(worker) {
        let result = match queued.command {
            WorkerCommand::Mark { sequence, task } => WorkerResult {
                sequence,
                worker,
                stolen: queued.stolen,
                outcome: scan(&task),
            },
            WorkerCommand::RefineCard {
                sequence,
                task,
                young,
            } => WorkerResult {
                sequence,
                worker,
                stolen: queued.stolen,
                outcome: refine_card(&task, &young),
            },
            WorkerCommand::Sweep {
                sequence,
                reference,
            } => WorkerResult {
                sequence,
                worker,
                stolen: queued.stolen,
                outcome: WorkerOutcome::Sweep(reference),
            },
            WorkerCommand::Evacuate {
                sequence,
                task,
                relocations,
            } => WorkerResult {
                sequence,
                worker,
                stolen: queued.stolen,
                outcome: evacuate(task, &relocations),
            },
        };
        if results.send(result).is_err() {
            break;
        }
    }
}

fn evacuate(
    mut task: EvacuationRewriteTask,
    relocations: &BTreeMap<ManagedReference, ManagedReference>,
) -> WorkerOutcome {
    let mut fields_updated = 0usize;
    for slot in &mut task.allocation.allocation.slots {
        if let SlotValue::Reference(Some(reference)) = slot
            && let Some(destination) = relocations.get(reference)
        {
            *reference = *destination;
            fields_updated = fields_updated.saturating_add(1);
        }
    }
    WorkerOutcome::Evacuated {
        destination: task.destination,
        allocation: task.allocation,
        fields_updated,
    }
}

fn refine_card(task: &CardRefinementTask, young: &BTreeSet<ManagedReference>) -> WorkerOutcome {
    let mut children = Vec::new();
    for slot in task.allocation.object_map.reference_slots() {
        match task.allocation.slots.get(slot.raw() as usize) {
            Some(SlotValue::Reference(Some(reference))) if young.contains(reference) => {
                children.push(*reference);
            }
            Some(SlotValue::Reference(_)) => {}
            Some(SlotValue::Scalar(_)) | None => {
                return WorkerOutcome::RefinedCard {
                    owner: task.owner,
                    children: Err(()),
                };
            }
        }
    }
    WorkerOutcome::RefinedCard {
        owner: task.owner,
        children: Ok(children),
    }
}

fn scan(task: &MarkTask) -> WorkerOutcome {
    let children = scan_slots(&task.slots);
    WorkerOutcome::Mark {
        reference: task.reference,
        large_object_scan_chunk: task.large_object_scan_chunk,
        children,
    }
}

pub(crate) fn scan_slots(slots: &[SlotValue]) -> Result<Vec<ManagedReference>, ()> {
    let mut children = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            SlotValue::Reference(Some(reference)) => children.push(*reference),
            SlotValue::Reference(None) => {}
            SlotValue::Scalar(_) => return Err(()),
        }
    }
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::{QueuedWorkerCommand, SharedWorkerQueues, WorkerCommand};
    use pop_runtime_interface::ManagedReference;

    #[test]
    fn idle_worker_steals_newest_peer_job_without_reordering_peer_fifo() {
        let queues = SharedWorkerQueues::new(2, 2);
        queues.submit(0, sweep(1)).expect("submit oldest peer job");
        queues.submit(0, sweep(2)).expect("submit newest peer job");

        let stolen = queues.take(1).expect("idle worker steals");
        assert!(stolen.stolen);
        assert_eq!(sequence(&stolen), 2);

        let local = queues.take(0).expect("owner retains oldest job");
        assert!(!local.stolen);
        assert_eq!(sequence(&local), 1);
    }

    #[test]
    fn worker_prefers_its_local_fifo_before_stealing() {
        let queues = SharedWorkerQueues::new(2, 2);
        queues.submit(0, sweep(1)).expect("submit peer job");
        queues.submit(1, sweep(2)).expect("submit local job");

        let local = queues.take(1).expect("take local job");
        assert!(!local.stolen);
        assert_eq!(sequence(&local), 2);

        let stolen = queues.take(1).expect("steal remaining peer job");
        assert!(stolen.stolen);
        assert_eq!(sequence(&stolen), 1);
    }

    fn sweep(sequence: u64) -> WorkerCommand {
        WorkerCommand::Sweep {
            sequence,
            reference: ManagedReference::new(sequence),
        }
    }

    fn sequence(command: &QueuedWorkerCommand) -> u64 {
        match &command.command {
            WorkerCommand::Sweep { sequence, .. } => *sequence,
            WorkerCommand::Mark { .. }
            | WorkerCommand::RefineCard { .. }
            | WorkerCommand::Evacuate { .. } => panic!("test queue contained a non-sweep command"),
        }
    }
}
