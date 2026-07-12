#[path = "../benches/workload.rs"]
mod workload;

use workload::{
    WorkloadConfiguration, WorkloadKind, run_allocation_pressure, run_managed_array,
    run_pin_pressure, run_rooted_chain, run_tiny_object_churn, run_workload,
};

#[test]
fn tiny_object_benchmark_has_deterministic_logical_work() {
    let counters = run_tiny_object_churn(WorkloadConfiguration {
        batches: 4,
        items_per_batch: 32,
        slots_per_object: 2,
        pressure_limit: 8,
    })
    .expect("valid benchmark workload");

    assert_eq!(counters.operations, 128);
    assert_eq!(counters.collections, 4);
    assert_eq!(counters.reclaimed_objects, 128);
    assert_eq!(counters.logical_peak_objects, 32);
    assert_eq!(counters.logical_peak_slots, 64);
    assert_eq!(counters.final_live_objects, 0);
    assert_eq!(counters.final_live_slots, 0);
}

#[test]
fn tiny_object_benchmark_rejects_empty_workloads() {
    assert!(
        run_tiny_object_churn(WorkloadConfiguration {
            batches: 0,
            items_per_batch: 32,
            slots_per_object: 2,
            pressure_limit: 8,
        })
        .is_err()
    );
    assert!(
        run_tiny_object_churn(WorkloadConfiguration {
            batches: 4,
            items_per_batch: 0,
            slots_per_object: 2,
            pressure_limit: 8,
        })
        .is_err()
    );
}

fn representative_configuration() -> WorkloadConfiguration {
    WorkloadConfiguration {
        batches: 2,
        items_per_batch: 8,
        slots_per_object: 2,
        pressure_limit: 8,
    }
}

#[test]
fn workload_inventory_is_closed_and_dispatches_after_harness_parsing() {
    let names: Vec<_> = WorkloadKind::ALL
        .into_iter()
        .map(WorkloadKind::name)
        .collect();
    assert_eq!(
        names,
        [
            "tiny_object_churn",
            "rooted_chain",
            "managed_array",
            "pin_pressure",
            "allocation_pressure",
        ]
    );
    for name in names {
        let workload = WorkloadKind::parse(name).expect("known harness-only workload name");
        assert_eq!(workload.name(), name);
    }
    assert!(WorkloadKind::parse("unknown").is_none());
    assert_eq!(
        run_workload(
            WorkloadKind::TinyObjectChurn,
            representative_configuration()
        )
        .expect("typed workload dispatch")
        .workload,
        "tiny_object_churn"
    );
}

#[test]
fn rooted_chain_benchmark_traces_then_reclaims_every_node() {
    let counters = run_rooted_chain(representative_configuration()).expect("rooted chain");

    assert_eq!(counters.allocations, 16);
    assert_eq!(counters.reference_stores, 14);
    assert_eq!(counters.root_transitions, 4);
    assert_eq!(counters.collections, 4);
    assert_eq!(counters.scanned_objects, 16);
    assert_eq!(counters.reclaimed_objects, 16);
    assert_eq!(counters.logical_peak_objects, 8);
    assert_eq!(counters.final_live_objects, 0);
}

#[test]
fn managed_array_benchmark_preserves_precise_elements() {
    let counters = run_managed_array(representative_configuration()).expect("managed array");

    assert_eq!(counters.allocations, 18);
    assert_eq!(counters.reference_stores, 16);
    assert_eq!(counters.root_transitions, 4);
    assert_eq!(counters.collections, 4);
    assert_eq!(counters.scanned_objects, 18);
    assert_eq!(counters.reclaimed_objects, 18);
    assert_eq!(counters.logical_peak_objects, 9);
    assert_eq!(counters.logical_peak_slots, 8);
}

#[test]
fn pin_benchmark_counts_scoped_pin_transitions() {
    let counters = run_pin_pressure(representative_configuration()).expect("pin pressure");

    assert_eq!(counters.allocations, 16);
    assert_eq!(counters.pin_transitions, 32);
    assert_eq!(counters.collections, 4);
    assert_eq!(counters.scanned_objects, 16);
    assert_eq!(counters.reclaimed_objects, 16);
    assert_eq!(counters.final_live_objects, 0);
}

#[test]
fn allocation_pressure_counts_automatic_collections() {
    let mut configuration = representative_configuration();
    configuration.batches = 8;
    let counters = run_allocation_pressure(configuration).expect("allocation pressure");

    assert_eq!(counters.allocations, 64);
    assert_eq!(counters.collections, 8);
    assert_eq!(counters.reclaimed_objects, 64);
    assert_eq!(counters.scanned_objects, 0);
    assert_eq!(counters.logical_peak_objects, 8);
    assert_eq!(counters.final_live_objects, 0);
}
