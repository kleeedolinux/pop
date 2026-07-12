mod workload;

use std::hint::black_box;
use std::time::Instant;

use workload::{WorkloadConfiguration, WorkloadKind, run_workload};

fn argument(name: &str, default: u32) -> u32 {
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
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                    }),
                "{name} must use only ASCII letters, digits, dash, underscore, or dot"
            );
            return value;
        }
    }
    default.to_owned()
}

fn main() {
    let samples = argument("--samples", 5);
    let profile = text_argument("--profile", "local-unlabeled");
    let selected = text_argument("--workload", "all");
    let workloads = if selected == "all" {
        WorkloadKind::ALL.to_vec()
    } else {
        vec![
            WorkloadKind::parse(&selected)
                .unwrap_or_else(|| panic!("unknown benchmark workload `{selected}`")),
        ]
    };
    let configuration = WorkloadConfiguration {
        batches: argument("--batches", 32),
        items_per_batch: argument("--items-per-batch", 2_048),
        slots_per_object: argument("--slots-per-object", 2),
        pressure_limit: argument("--pressure-limit", 256),
    };
    let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);

    for workload in workloads {
        for sample in 1..=samples {
            let started = Instant::now();
            let counters = black_box(
                run_workload(workload, black_box(configuration))
                    .expect("valid benchmark configuration must execute"),
            );
            let elapsed_nanoseconds = started.elapsed().as_nanos();
            let operations = u128::from(counters.operations.max(1));
            let nanoseconds_per_operation = elapsed_nanoseconds / operations;
            let fractional_nanoseconds =
                (elapsed_nanoseconds % operations).saturating_mul(1_000) / operations;

            println!(
                "schema=pop-runtime-benchmark-v1\tprofile={profile}\ttarget_architecture={}\ttarget_operating_system={}\tbuild_profile=bench-optimized\tcollector_stage=BootstrapPreciseStopTheWorld\tworkload={}\tgraph_shape={}\troots={}\tsamples={samples}\tsample={sample}\tbatches={}\titems_per_batch={}\tslots_per_object={}\tpressure_limit={}\toperations={}\tallocations={}\treference_stores={}\troot_transitions={}\tpin_transitions={}\telapsed_nanoseconds={elapsed_nanoseconds}\tnanoseconds_per_operation={nanoseconds_per_operation}.{fractional_nanoseconds:03}\tavailable_parallelism={available_parallelism}\tlogical_peak_objects={}\tlogical_peak_slots={}\tcollections={}\treclaimed_objects={}\tscanned_objects={}\tfinal_live_objects={}\tfinal_live_slots={}",
                std::env::consts::ARCH,
                std::env::consts::OS,
                counters.workload,
                counters.graph_shape,
                counters.roots,
                configuration.batches,
                configuration.items_per_batch,
                configuration.slots_per_object,
                configuration.pressure_limit,
                counters.operations,
                counters.allocations,
                counters.reference_stores,
                counters.root_transitions,
                counters.pin_transitions,
                counters.logical_peak_objects,
                counters.logical_peak_slots,
                counters.collections,
                counters.reclaimed_objects,
                counters.scanned_objects,
                counters.final_live_objects,
                counters.final_live_slots,
            );
        }
    }
}
