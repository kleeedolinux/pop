use pop_runtime_collector::RelocationRuntime;
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, ObjectSlot, RootPublication, RootSlot,
    RuntimeAdapter, RuntimeFailure, RuntimeTypeId, SafePointId, StackMap,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelocationWorkloadConfiguration {
    pub batches: u32,
    pub items_per_batch: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelocationWorkloadCounters {
    pub operations: u64,
    pub allocations: u64,
    pub reference_stores: u64,
    pub collections: u64,
    pub relocated_roots: u64,
    pub reclaimed_objects: u64,
    pub scanned_objects: u64,
    pub logical_peak_objects: u64,
    pub final_live_objects: u64,
}

/// Copies one rooted chain per batch, then removes its root and reclaims it.
///
/// # Errors
///
/// Returns a runtime invariant failure for empty configuration or any failed
/// allocation, edge store, root publication, or collection.
pub fn run_relocation_churn(
    configuration: RelocationWorkloadConfiguration,
) -> Result<RelocationWorkloadCounters, RuntimeFailure> {
    if configuration.batches == 0 || configuration.items_per_batch == 0 {
        return Err(RuntimeFailure::runtime_invariant());
    }
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(1),
        AllocationClass::NurseryEligible,
        ObjectMap::new(1, vec![ObjectSlot::new(0)])
            .map_err(|_| RuntimeFailure::runtime_invariant())?,
    );
    let mut runtime = RelocationRuntime::new();
    let mut reference_stores = 0_u64;
    let mut relocated_roots = 0_u64;
    let mut logical_peak_objects = 0_u64;

    for batch in 0..configuration.batches {
        let mut nodes = Vec::with_capacity(configuration.items_per_batch as usize);
        for _ in 0..configuration.items_per_batch {
            nodes.push(runtime.allocate_object(&request)?);
        }
        for pair in nodes.windows(2) {
            runtime.store_reference(pair[0], ObjectSlot::new(0), Some(pair[1]))?;
            reference_stores = reference_stores.saturating_add(1);
        }
        logical_peak_objects =
            logical_peak_objects.max(u64::try_from(runtime.object_count()).unwrap_or(u64::MAX));
        let original_root = nodes[0];
        let mut publication = RootPublication::new(
            StackMap::new(SafePointId::new(batch * 2), vec![RootSlot::new(0)])
                .map_err(|_| RuntimeFailure::runtime_invariant())?,
            vec![Some(original_root)],
        )
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
        force_minor(&mut runtime, &mut publication)?;
        if publication.managed_references().next() != Some(original_root) {
            relocated_roots = relocated_roots.saturating_add(1);
        }
        for (_, value) in publication.root_values_mut() {
            *value = None;
        }
        force_minor(&mut runtime, &mut publication)?;
    }

    let metrics = runtime.metrics();
    Ok(RelocationWorkloadCounters {
        operations: metrics
            .allocations()
            .saturating_add(reference_stores)
            .saturating_add(metrics.collections()),
        allocations: metrics.allocations(),
        reference_stores,
        collections: metrics.collections(),
        relocated_roots,
        reclaimed_objects: metrics.reclaimed_objects(),
        scanned_objects: metrics.scanned_objects(),
        logical_peak_objects,
        final_live_objects: u64::try_from(runtime.object_count()).unwrap_or(u64::MAX),
    })
}

fn force_minor(
    runtime: &mut RelocationRuntime,
    roots: &mut RootPublication,
) -> Result<(), RuntimeFailure> {
    runtime.request_minor_collection();
    runtime
        .safe_point(roots)?
        .collection()
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    Ok(())
}
