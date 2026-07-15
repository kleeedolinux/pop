use std::sync::{Arc, Barrier};
use std::thread;

use pop_runtime_collector::{
    ParallelSchedulerLocalConfigError, ParallelSchedulerLocalRuntime, SchedulerId,
};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, RootPublication,
    RootSlot, RuntimeTypeId, SafePointId, StackMap,
};

fn nursery_object() -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(111),
        AllocationClass::NurseryEligible,
        ObjectMap::new(0, Vec::new()).expect("object map"),
    )
}

fn one_root(id: u32, reference: ManagedReference) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), vec![RootSlot::new(0)]).expect("stack map"),
        vec![Some(reference)],
    )
    .expect("root publication")
}

#[test]
fn parallel_scheduler_inventory_fails_closed() {
    assert!(matches!(
        ParallelSchedulerLocalRuntime::new([]),
        Err(ParallelSchedulerLocalConfigError::Empty)
    ));
    assert!(matches!(
        ParallelSchedulerLocalRuntime::new([SchedulerId::new(0)]),
        Err(ParallelSchedulerLocalConfigError::InvalidScheduler(scheduler))
            if scheduler == SchedulerId::new(0)
    ));
    assert!(matches!(
        ParallelSchedulerLocalRuntime::new([SchedulerId::new(1), SchedulerId::new(1)]),
        Err(ParallelSchedulerLocalConfigError::DuplicateScheduler(scheduler))
            if scheduler == SchedulerId::new(1)
    ));
}

#[test]
fn scheduler_contexts_allocate_concurrently_with_disjoint_tokens_and_tlabs() {
    let runtime = Arc::new(
        ParallelSchedulerLocalRuntime::new([SchedulerId::new(1), SchedulerId::new(2)])
            .expect("parallel local runtime"),
    );
    let rendezvous = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for scheduler in [SchedulerId::new(1), SchedulerId::new(2)] {
        let runtime = Arc::clone(&runtime);
        let rendezvous = Arc::clone(&rendezvous);
        workers.push(thread::spawn(move || {
            runtime
                .with_scheduler(scheduler, |context| {
                    rendezvous.wait();
                    (0..64)
                        .map(|_| context.allocate_object(&nursery_object()))
                        .collect::<Result<Vec<_>, _>>()
                })
                .expect("scheduler allocation")
        }));
    }
    let first = workers.remove(0).join().expect("scheduler one");
    let second = workers.remove(0).join().expect("scheduler two");

    assert!(first.iter().all(|reference| reference.raw() >> 32 == 1));
    assert!(second.iter().all(|reference| reference.raw() >> 32 == 2));
    assert_eq!(runtime.telemetry().maximum_parallel_operations(), 2);
    assert_eq!(runtime.scheduler_tlab_refills(SchedulerId::new(1)), Some(1));
    assert_eq!(runtime.scheduler_tlab_refills(SchedulerId::new(2)), Some(1));
}

#[test]
fn scheduler_minor_evacuations_run_concurrently_and_remain_scoped() {
    let runtime = Arc::new(
        ParallelSchedulerLocalRuntime::new([SchedulerId::new(3), SchedulerId::new(4)])
            .expect("parallel local runtime"),
    );
    let rendezvous = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for scheduler in [SchedulerId::new(3), SchedulerId::new(4)] {
        let runtime = Arc::clone(&runtime);
        let rendezvous = Arc::clone(&rendezvous);
        workers.push(thread::spawn(move || {
            runtime
                .with_scheduler(scheduler, |context| {
                    let live = context.allocate_object(&nursery_object())?;
                    let dead = context.allocate_object(&nursery_object())?;
                    context.request_minor_collection();
                    rendezvous.wait();
                    let mut roots = one_root(scheduler.raw(), live);
                    let outcome = context.safe_point(&mut roots)?;
                    let relocated = roots
                        .managed_references()
                        .next()
                        .expect("relocated live root");
                    Ok((
                        live,
                        dead,
                        relocated,
                        outcome.collection().expect("minor collection"),
                    ))
                })
                .expect("scheduler evacuation")
        }));
    }
    let (first_live, first_dead, first_relocated, first_statistics) =
        workers.remove(0).join().expect("scheduler three");
    let (second_live, second_dead, second_relocated, second_statistics) =
        workers.remove(0).join().expect("scheduler four");

    assert_eq!(first_statistics.reclaimed_objects(), 1);
    assert_eq!(second_statistics.reclaimed_objects(), 1);
    assert_ne!(first_relocated, first_live);
    assert_ne!(second_relocated, second_live);
    assert!(!runtime.scheduler_contains(SchedulerId::new(3), first_live));
    assert!(!runtime.scheduler_contains(SchedulerId::new(3), first_dead));
    assert!(runtime.scheduler_contains(SchedulerId::new(3), first_relocated));
    assert!(!runtime.scheduler_contains(SchedulerId::new(4), second_live));
    assert!(!runtime.scheduler_contains(SchedulerId::new(4), second_dead));
    assert!(runtime.scheduler_contains(SchedulerId::new(4), second_relocated));
    assert_eq!(runtime.telemetry().maximum_parallel_operations(), 2);
}
