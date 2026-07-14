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
    assert_eq!(SCHEDULER_BENCHMARK_SCHEMA, "pop-scheduler-benchmark-v1");
    let names: Vec<_> = SchedulerWorkload::ALL
        .into_iter()
        .map(SchedulerWorkload::name)
        .collect();
    assert_eq!(
        names,
        [
            "task_control",
            "ready_polls",
            "burst_injection",
            "hot_queue_steal",
            "suspended_frames",
            "timer_fan_out",
            "external_event_fan_out",
            "blocking_saturation",
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
    assert_eq!(counters.stale_ready_entries, 0);
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
