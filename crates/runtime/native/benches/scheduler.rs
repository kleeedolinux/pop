mod scheduler_workload;

use std::hint::black_box;
use std::time::Instant;

use scheduler_workload::{
    SCHEDULER_BENCHMARK_SCHEMA, SchedulerBenchmarkConfiguration, SchedulerWorkload,
    run_scheduler_workload,
};

fn argument(name: &str, default: usize) -> usize {
    let mut arguments = std::env::args();
    while let Some(argument) = arguments.next() {
        if argument == name {
            return arguments
                .next()
                .unwrap_or_else(|| panic!("missing value for {name}"))
                .parse()
                .unwrap_or_else(|_| panic!("invalid value for {name}"));
        }
    }
    default
}

fn text_argument(name: &str, default: &str) -> String {
    let mut arguments = std::env::args();
    while let Some(argument) = arguments.next() {
        if argument == name {
            let value = arguments
                .next()
                .unwrap_or_else(|| panic!("missing value for {name}"));
            assert!(
                !value.is_empty()
                    && value.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '-' | '_' | '.' | ',')
                    }),
                "{name} contains an unsupported character"
            );
            return value;
        }
    }
    default.to_owned()
}

fn worker_profiles(selected: &str, task_count: usize) -> Vec<usize> {
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    let mut workers = if selected == "standard" {
        vec![1, 2, 4, available]
    } else {
        selected
            .split(',')
            .map(|value| {
                if value == "available" {
                    available
                } else {
                    value
                        .parse()
                        .unwrap_or_else(|_| panic!("invalid worker profile `{value}`"))
                }
            })
            .collect()
    };
    workers.retain(|workers| *workers > 0 && *workers <= task_count);
    workers.sort_unstable();
    workers.dedup();
    assert!(!workers.is_empty(), "no worker profile fits the task count");
    workers
}

fn main() {
    let samples = argument("--samples", 5);
    let tasks = argument("--tasks", 8_192);
    let polls_per_task = argument("--polls-per-task", 16);
    let profile = text_argument("--profile", "local-unlabeled");
    let selected_workload = text_argument("--workload", "all");
    let workloads = if selected_workload == "all" {
        SchedulerWorkload::ALL.to_vec()
    } else {
        vec![
            SchedulerWorkload::parse(&selected_workload)
                .unwrap_or_else(|| panic!("unknown scheduler workload `{selected_workload}`")),
        ]
    };
    let workers = worker_profiles(&text_argument("--workers", "standard"), tasks);
    let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);

    for worker_count in workers {
        let configuration = SchedulerBenchmarkConfiguration {
            workers: worker_count,
            tasks,
            polls_per_task,
        };
        for workload in &workloads {
            for sample in 1..=samples {
                let started = Instant::now();
                let counters = black_box(
                    run_scheduler_workload(*workload, black_box(configuration))
                        .expect("valid scheduler benchmark must preserve its invariants"),
                );
                let elapsed_nanoseconds = started.elapsed().as_nanos();
                let operations = u128::from(counters.operations.max(1));
                let nanoseconds_per_operation = elapsed_nanoseconds / operations;
                let fractional_nanoseconds =
                    (elapsed_nanoseconds % operations).saturating_mul(1_000) / operations;

                println!(
                    "schema={}\tprofile={profile}\ttarget_architecture={}\ttarget_operating_system={}\tbuild_profile=bench-optimized\tscheduler_stage=bounded-synchronized-reference\tworkload={}\tworkers={}\tavailable_parallelism={available_parallelism}\tsamples={samples}\tsample={sample}\ttasks={}\tpolls_per_task={polls_per_task}\toperations={}\tchecksum={}\telapsed_nanoseconds={elapsed_nanoseconds}\tnanoseconds_per_operation={nanoseconds_per_operation}.{fractional_nanoseconds:03}\tlatency_scope=initial-ready-to-first-poll\tlatency_p50_nanoseconds={}\tlatency_p95_nanoseconds={}\tlatency_p99_nanoseconds={}\tlatency_p999_nanoseconds={}\tlatency_max_nanoseconds={}\tpolls={}\tcompletions={}\tsuspensions={}\twake_requests={}\ttasks_stolen={}\tblocking_submissions={}\ttimers_delivered={}\texternal_events_delivered={}\tlocal_queue_depth={}\tmaximum_local_queue_depth={}\tinjection_queue_depth={}\tmaximum_injection_queue_depth={}\tblocking_queue_depth={}\tmaximum_blocking_queue_depth={}\tactive_blocking_operations={}\tmaximum_active_blocking_operations={}\tsteal_searches={}\tsteal_victims_examined={}\tsteal_successes={}\tsteal_failures={}\tmaximum_stolen_batch={}\tworker_starts={}\tworker_parks={}\tworker_unparks={}\tworker_stops={}\tstale_ready_entries={}",
                    SCHEDULER_BENCHMARK_SCHEMA,
                    std::env::consts::ARCH,
                    std::env::consts::OS,
                    counters.workload,
                    counters.workers,
                    counters.tasks,
                    counters.operations,
                    counters.checksum,
                    counters.first_poll_latency_p50_nanoseconds,
                    counters.first_poll_latency_p95_nanoseconds,
                    counters.first_poll_latency_p99_nanoseconds,
                    counters.first_poll_latency_p999_nanoseconds,
                    counters.first_poll_latency_max_nanoseconds,
                    counters.polls,
                    counters.completions,
                    counters.suspensions,
                    counters.wake_requests,
                    counters.tasks_stolen,
                    counters.blocking_submissions,
                    counters.timers_delivered,
                    counters.external_events_delivered,
                    counters.local_queue_depth,
                    counters.maximum_local_queue_depth,
                    counters.injection_queue_depth,
                    counters.maximum_injection_queue_depth,
                    counters.blocking_queue_depth,
                    counters.maximum_blocking_queue_depth,
                    counters.active_blocking_operations,
                    counters.maximum_active_blocking_operations,
                    counters.steal_searches,
                    counters.steal_victims_examined,
                    counters.steal_successes,
                    counters.steal_failures,
                    counters.maximum_stolen_batch,
                    counters.worker_starts,
                    counters.worker_parks,
                    counters.worker_unparks,
                    counters.worker_stops,
                    counters.stale_ready_entries,
                );
            }
        }
    }
}
