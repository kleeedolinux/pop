//! Per-worker bounded queues and deterministic result collection.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use pop_runtime_interface::{ManagedReference, RuntimeFailure};

use crate::heap::{Allocation, SlotValue};

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
    Shutdown,
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
}

struct WorkerResult {
    sequence: u64,
    worker: usize,
    outcome: WorkerOutcome,
}

pub(crate) struct MarkResult {
    pub(crate) reference: ManagedReference,
    pub(crate) large_object_scan_chunk: Option<LargeObjectScanChunk>,
    pub(crate) children: Vec<ManagedReference>,
}

pub(crate) struct BackgroundWorkerPool {
    senders: Vec<SyncSender<WorkerCommand>>,
    results: Receiver<WorkerResult>,
    threads: Vec<JoinHandle<()>>,
    next_worker: usize,
    next_sequence: u64,
    workers_used: BTreeSet<usize>,
    telemetry: BackgroundWorkerTelemetry,
}

impl BackgroundWorkerPool {
    pub(crate) fn new(config: BackgroundWorkerConfig) -> Result<Self, BackgroundWorkerStartError> {
        let (result_sender, results) = mpsc::channel();
        let mut senders: Vec<SyncSender<WorkerCommand>> = Vec::with_capacity(config.worker_count);
        let mut threads: Vec<JoinHandle<()>> = Vec::with_capacity(config.worker_count);
        for worker in 0..config.worker_count {
            let (sender, receiver) = mpsc::sync_channel(config.queue_capacity);
            let worker_results = result_sender.clone();
            let Ok(handle) = thread::Builder::new()
                .name(format!("pop-gc-{worker}"))
                .spawn(move || worker_loop(worker, &receiver, &worker_results))
            else {
                for sender in &senders {
                    let _ = sender.send(WorkerCommand::Shutdown);
                }
                for thread in threads {
                    let _ = thread.join();
                }
                return Err(BackgroundWorkerStartError::ThreadSpawn);
            };
            senders.push(sender);
            threads.push(handle);
        }
        drop(result_sender);
        Ok(Self {
            senders,
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
                WorkerOutcome::RefinedCard { .. } | WorkerOutcome::Sweep(_) => {
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
                WorkerOutcome::Mark { .. } | WorkerOutcome::Sweep(_) => {
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
                WorkerOutcome::Mark { .. } | WorkerOutcome::RefinedCard { .. } => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            }
        }
        self.complete_batch(count);
        Ok(swept)
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
        self.next_worker = (self.next_worker + 1) % self.senders.len();
        self.senders[worker]
            .send(command)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
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
        for sender in &self.senders {
            let _ = sender.send(WorkerCommand::Shutdown);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn worker_loop(
    worker: usize,
    receiver: &Receiver<WorkerCommand>,
    results: &mpsc::Sender<WorkerResult>,
) {
    while let Ok(command) = receiver.recv() {
        let result = match command {
            WorkerCommand::Mark { sequence, task } => WorkerResult {
                sequence,
                worker,
                outcome: scan(&task),
            },
            WorkerCommand::RefineCard {
                sequence,
                task,
                young,
            } => WorkerResult {
                sequence,
                worker,
                outcome: refine_card(&task, &young),
            },
            WorkerCommand::Sweep {
                sequence,
                reference,
            } => WorkerResult {
                sequence,
                worker,
                outcome: WorkerOutcome::Sweep(reference),
            },
            WorkerCommand::Shutdown => break,
        };
        if results.send(result).is_err() {
            break;
        }
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
