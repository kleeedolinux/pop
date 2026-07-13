mod compilation_workload;

use std::hint::black_box;
use std::time::Instant;

use compilation_workload::{
    CompilationStage, CompilationWorkloadConfiguration, CompilationWorkloadKind, prepare_workload,
};

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

fn selected_workloads(selected: &str) -> Vec<CompilationWorkloadKind> {
    if selected == "all" {
        CompilationWorkloadKind::ALL.to_vec()
    } else {
        vec![
            CompilationWorkloadKind::parse(selected)
                .unwrap_or_else(|| panic!("unknown benchmark workload `{selected}`")),
        ]
    }
}

fn selected_stages(selected: &str) -> Vec<CompilationStage> {
    if selected == "all" {
        CompilationStage::ALL.to_vec()
    } else {
        vec![
            CompilationStage::parse(selected)
                .unwrap_or_else(|| panic!("unknown compilation stage `{selected}`")),
        ]
    }
}

fn main() {
    let samples = argument("--samples", 5);
    let profile = text_argument("--profile", "local-unlabeled");
    let workloads = selected_workloads(&text_argument("--workload", "all"));
    let stages = selected_stages(&text_argument("--stage", "all"));
    let configuration = CompilationWorkloadConfiguration {
        modules: argument("--modules", 8),
        functions_per_module: argument("--functions-per-module", 32),
        statements_per_function: argument("--statements-per-function", 16),
    };
    let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);

    for workload in workloads {
        let prepared = prepare_workload(workload, configuration)
            .unwrap_or_else(|error| panic!("could not prepare {}: {error}", workload.name()));
        for stage in &stages {
            for sample in 1..=samples {
                let started = Instant::now();
                let observation = black_box(
                    prepared
                        .run_stage(black_box(*stage))
                        .expect("prepared compilation stage must succeed"),
                );
                let elapsed_nanoseconds = started.elapsed().as_nanos();
                println!(
                    "schema=pop-compiler-benchmark-v1\tprofile={profile}\ttarget_architecture={}\ttarget_operating_system={}\tbuild_profile=bench-optimized\tworkload={}\tstage={}\tsamples={samples}\tsample={sample}\tmodules={}\tfunctions={}\tstatements={}\tsource_bytes={}\tsemantic_items={}\toutput_bytes={}\telapsed_nanoseconds={elapsed_nanoseconds}\tavailable_parallelism={available_parallelism}",
                    std::env::consts::ARCH,
                    std::env::consts::OS,
                    workload.name(),
                    stage.name(),
                    prepared.logical_modules(),
                    prepared.logical_functions(),
                    prepared.logical_statements(),
                    prepared.source_bytes(),
                    observation.semantic_items,
                    observation.output_bytes,
                );
            }
        }
    }
}
