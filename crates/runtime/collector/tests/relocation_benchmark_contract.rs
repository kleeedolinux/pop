#[path = "../benches/relocation_workload.rs"]
mod relocation_workload;

use relocation_workload::{RelocationWorkloadConfiguration, run_relocation_churn};

#[test]
fn relocation_workload_counts_copy_and_reclaim_cycles_deterministically() {
    let counters = run_relocation_churn(RelocationWorkloadConfiguration {
        batches: 2,
        items_per_batch: 4,
    })
    .expect("relocation workload");

    assert_eq!(counters.operations, 18);
    assert_eq!(counters.allocations, 8);
    assert_eq!(counters.reference_stores, 6);
    assert_eq!(counters.collections, 4);
    assert_eq!(counters.relocated_roots, 2);
    assert_eq!(counters.reclaimed_objects, 8);
    assert_eq!(counters.scanned_objects, 8);
    assert_eq!(counters.logical_peak_objects, 4);
    assert_eq!(counters.final_live_objects, 0);
}

#[test]
fn relocation_workload_rejects_empty_configuration() {
    assert!(
        run_relocation_churn(RelocationWorkloadConfiguration {
            batches: 0,
            items_per_batch: 4,
        })
        .is_err()
    );
    assert!(
        run_relocation_churn(RelocationWorkloadConfiguration {
            batches: 2,
            items_per_batch: 0,
        })
        .is_err()
    );
}
