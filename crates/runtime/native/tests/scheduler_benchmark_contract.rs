#[path = "../benches/scheduler_workload.rs"]
mod scheduler_workload;

use scheduler_workload::{
    SCHEDULER_BENCHMARK_SCHEMA, SchedulerBenchmarkConfiguration, SchedulerWorkload,
    run_scheduler_workload,
};

fn representative_configuration() -> SchedulerBenchmarkConfiguration {
    SchedulerBenchmarkConfiguration {
        workers: 2,
        tasks: 8,
        polls_per_task: 3,
    }
}

#[test]
fn scheduler_benchmark_inventory_is_closed_and_typed() {
    assert_eq!(SCHEDULER_BENCHMARK_SCHEMA, "pop-scheduler-benchmark-v3");
    let names: Vec<_> = SchedulerWorkload::ALL
        .into_iter()
        .map(SchedulerWorkload::name)
        .collect();
    assert_eq!(
        names,
        [
            "task_control",
            "ready_polls",
            "local_wake",
            "foreign_wake",
            "burst_injection",
            "hot_queue_steal",
            "ping_pong",
            "steal_storm",
            "suspended_frames",
            "timer_fan_out",
            "external_event_fan_out",
            "continuous_event_fairness",
            "blocking_saturation",
            "scheduler_gc_interaction",
        ]
    );
    for name in names {
        assert_eq!(
            SchedulerWorkload::parse(name).map(SchedulerWorkload::name),
            Some(name)
        );
    }
    assert!(SchedulerWorkload::parse("unknown").is_none());
}

#[test]
fn specialized_scheduler_profiles_preserve_their_typed_work() {
    let configuration = representative_configuration();
    let local = run_scheduler_workload(SchedulerWorkload::LocalWake, configuration)
        .expect("local wake profile");
    assert_eq!(local.polls, local.operations);
    assert_eq!(local.ready_delay_samples, local.polls);

    let foreign = run_scheduler_workload(SchedulerWorkload::ForeignWake, configuration)
        .expect("foreign wake profile");
    assert_eq!(foreign.wake_requests, foreign.tasks);

    let ping_pong = run_scheduler_workload(SchedulerWorkload::PingPong, configuration)
        .expect("ping-pong profile");
    assert!(ping_pong.polls >= ping_pong.operations);
    assert_eq!(ping_pong.completions, ping_pong.tasks);

    let events = run_scheduler_workload(SchedulerWorkload::ContinuousEventFairness, configuration)
        .expect("continuous event fairness profile");
    assert!(events.external_events_delivered > 0);
    assert_eq!(events.completions, events.tasks);

    let gc = run_scheduler_workload(SchedulerWorkload::SchedulerGcInteraction, configuration)
        .expect("scheduler/GC interaction profile");
    assert_eq!(gc.completions, gc.tasks);
    assert_eq!(gc.polls, gc.operations);
}

#[test]
fn ready_poll_benchmark_preserves_exact_logical_work_and_checksum() {
    let counters = run_scheduler_workload(
        SchedulerWorkload::ReadyPolls,
        representative_configuration(),
    )
    .expect("valid ready-poll benchmark");

    assert_eq!(counters.tasks, 8);
    assert_eq!(counters.operations, 24);
    assert_eq!(counters.polls, 24);
    assert_eq!(counters.completions, 8);
    assert_eq!(counters.checksum, 108);
    assert_eq!(counters.local_queue_depth, 0);
    assert_eq!(counters.injection_queue_depth, 0);
    assert_eq!(counters.blocking_queue_depth, 0);
    assert!(counters.maximum_local_queue_depth > 0);
    assert_eq!(counters.worker_starts, 2);
    assert_eq!(counters.worker_stops, 2);
    assert_eq!(counters.stale_ready_entries, 0);
    assert_eq!(counters.ready_delay_samples, counters.polls);
    assert!(counters.ready_delay_p50_work_units <= counters.ready_delay_p99_work_units);
    assert!(counters.ready_delay_p99_work_units <= counters.ready_delay_max_work_units);
    assert!(matches!(
        counters.resource_counter_source,
        "linux-procfs" | "unavailable"
    ));
}

#[test]
fn every_scheduler_workload_completes_without_lost_or_duplicate_tasks() {
    for workload in SchedulerWorkload::ALL {
        let counters = run_scheduler_workload(workload, representative_configuration())
            .unwrap_or_else(|error| panic!("{} failed: {error:?}", workload.name()));
        assert_eq!(counters.tasks, 8, "{} task count", workload.name());
        assert_eq!(counters.completions, 8, "{} completions", workload.name());
        assert_eq!(
            counters.stale_ready_entries,
            0,
            "{} stale work",
            workload.name()
        );
        assert_ne!(counters.checksum, 0, "{} checksum", workload.name());
    }
}

#[test]
fn scheduler_benchmark_rejects_zero_and_incoherent_bounds() {
    for configuration in [
        SchedulerBenchmarkConfiguration {
            workers: 0,
            ..representative_configuration()
        },
        SchedulerBenchmarkConfiguration {
            tasks: 0,
            ..representative_configuration()
        },
        SchedulerBenchmarkConfiguration {
            polls_per_task: 0,
            ..representative_configuration()
        },
        SchedulerBenchmarkConfiguration {
            workers: 9,
            tasks: 8,
            polls_per_task: 1,
        },
    ] {
        assert!(run_scheduler_workload(SchedulerWorkload::ReadyPolls, configuration).is_err());
    }
}
