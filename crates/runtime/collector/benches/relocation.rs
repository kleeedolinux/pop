mod relocation_workload;

use std::hint::black_box;
use std::time::Instant;

use relocation_workload::{RelocationWorkloadConfiguration, run_relocation_churn};

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

fn profile() -> String {
    let mut arguments = std::env::args();
    while let Some(argument) = arguments.next() {
        if argument == "--profile" {
            let value = arguments.next().expect("missing value for --profile");
            assert!(
                !value.is_empty()
                    && value.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                    }),
                "profile must use only ASCII letters, digits, dash, underscore, or dot"
            );
            return value;
        }
    }
    "local-unlabeled".to_owned()
}

fn main() {
    let samples = argument("--samples", 5);
    let profile = profile();
    let configuration = RelocationWorkloadConfiguration {
        batches: argument("--batches", 32),
        items_per_batch: argument("--items-per-batch", 2_048),
    };
    let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);

    for sample in 1..=samples {
        let started = Instant::now();
        let counters = black_box(
            run_relocation_churn(black_box(configuration))
                .expect("valid relocation benchmark configuration must execute"),
        );
        let elapsed_nanoseconds = started.elapsed().as_nanos();
        let operations = u128::from(counters.operations.max(1));
        let nanoseconds_per_operation = elapsed_nanoseconds / operations;
        let fractional_nanoseconds =
            (elapsed_nanoseconds % operations).saturating_mul(1_000) / operations;

        println!(
            "schema=pop-runtime-benchmark-v1\tprofile={profile}\ttarget_architecture={}\ttarget_operating_system={}\tbuild_profile=bench-optimized\tcollector_stage=RelocationConformance\tworkload=relocation_churn\tgraph_shape=rooted_reference_chain\troots=1\tsamples={samples}\tsample={sample}\tbatches={}\titems_per_batch={}\toperations={}\tallocations={}\treference_stores={}\trelocated_roots={}\telapsed_nanoseconds={elapsed_nanoseconds}\tnanoseconds_per_operation={nanoseconds_per_operation}.{fractional_nanoseconds:03}\tavailable_parallelism={available_parallelism}\tlogical_peak_objects={}\tlogical_peak_slots={}\tcollections={}\treclaimed_objects={}\tscanned_objects={}\tfinal_live_objects={}\tfinal_live_slots=0",
            std::env::consts::ARCH,
            std::env::consts::OS,
            configuration.batches,
            configuration.items_per_batch,
            counters.operations,
            counters.allocations,
            counters.reference_stores,
            counters.relocated_roots,
            counters.logical_peak_objects,
            counters.logical_peak_objects,
            counters.collections,
            counters.reclaimed_objects,
            counters.scanned_objects,
            counters.final_live_objects,
        );
    }
}
