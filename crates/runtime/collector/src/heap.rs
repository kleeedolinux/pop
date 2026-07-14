//! Bootstrap heap state and public construction/query surface.

use std::collections::BTreeMap;

use pop_runtime_interface::{
    AllocationClass, ArrayElementMap, ManagedReference, ObjectMap, PinHandle, RootHandle,
    RuntimeTypeId,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeapLimits {
    pub(crate) maximum_objects: usize,
    pub(crate) maximum_slots: usize,
}

impl HeapLimits {
    #[must_use]
    pub const fn new(maximum_objects: usize, maximum_slots: usize) -> Self {
        Self {
            maximum_objects,
            maximum_slots,
        }
    }

    #[must_use]
    pub const fn maximum_objects(self) -> usize {
        self.maximum_objects
    }

    #[must_use]
    pub const fn maximum_slots(self) -> usize {
        self.maximum_slots
    }
}

impl Default for HeapLimits {
    fn default() -> Self {
        Self::new(usize::MAX, usize::MAX)
    }
}

/// Saturating implementation telemetry for the Stage-1 collector instance.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CollectorMetrics {
    allocations: u64,
    collections: u64,
    reclaimed_objects: u64,
    scanned_objects: u64,
}

impl CollectorMetrics {
    #[must_use]
    pub const fn new(
        allocations: u64,
        collections: u64,
        reclaimed_objects: u64,
        scanned_objects: u64,
    ) -> Self {
        Self {
            allocations,
            collections,
            reclaimed_objects,
            scanned_objects,
        }
    }

    #[must_use]
    pub const fn allocations(self) -> u64 {
        self.allocations
    }

    #[must_use]
    pub const fn collections(self) -> u64 {
        self.collections
    }

    #[must_use]
    pub const fn reclaimed_objects(self) -> u64 {
        self.reclaimed_objects
    }

    #[must_use]
    pub const fn scanned_objects(self) -> u64 {
        self.scanned_objects
    }

    pub(crate) fn record_allocation(&mut self) {
        self.allocations = self.allocations.saturating_add(1);
    }

    pub(crate) fn rollback_allocation(&mut self) {
        self.allocations = self.allocations.saturating_sub(1);
    }

    pub(crate) fn record_collection(&mut self, reclaimed: u64, scanned: u64) {
        self.collections = self.collections.saturating_add(1);
        self.reclaimed_objects = self.reclaimed_objects.saturating_add(reclaimed);
        self.scanned_objects = self.scanned_objects.saturating_add(scanned);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SlotValue {
    Scalar(u64),
    Reference(Option<ManagedReference>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllocationKind {
    Object,
    Array(ArrayElementMap),
    Table,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Allocation {
    pub(crate) kind: AllocationKind,
    pub(crate) type_id: RuntimeTypeId,
    pub(crate) class: AllocationClass,
    pub(crate) object_map: ObjectMap,
    pub(crate) slots: Vec<SlotValue>,
}

pub struct BootstrapRuntime {
    pub(crate) objects: BTreeMap<ManagedReference, Allocation>,
    pub(crate) roots: BTreeMap<RootHandle, ManagedReference>,
    pub(crate) pins: BTreeMap<PinHandle, ManagedReference>,
    pub(crate) next_reference: u64,
    pub(crate) next_root: u64,
    pub(crate) next_pin: u64,
    pub(crate) slot_count: usize,
    pub(crate) limits: HeapLimits,
    pub(crate) collection_requested: bool,
    pub(crate) metrics: CollectorMetrics,
}

impl BootstrapRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(HeapLimits::default())
    }

    #[must_use]
    pub fn with_limits(limits: HeapLimits) -> Self {
        Self {
            objects: BTreeMap::new(),
            roots: BTreeMap::new(),
            pins: BTreeMap::new(),
            next_reference: 1,
            next_root: 1,
            next_pin: 1,
            slot_count: 0,
            limits,
            collection_requested: false,
            metrics: CollectorMetrics::default(),
        }
    }

    #[must_use]
    pub const fn limits(&self) -> HeapLimits {
        self.limits
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    #[must_use]
    pub const fn slot_count(&self) -> usize {
        self.slot_count
    }

    #[must_use]
    pub const fn collection_requested(&self) -> bool {
        self.collection_requested
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        self.objects.contains_key(&reference)
    }

    #[must_use]
    pub fn allocation_type(&self, reference: ManagedReference) -> Option<RuntimeTypeId> {
        self.objects
            .get(&reference)
            .map(|allocation| allocation.type_id)
    }

    #[must_use]
    pub fn allocation_class(&self, reference: ManagedReference) -> Option<AllocationClass> {
        self.objects
            .get(&reference)
            .map(|allocation| allocation.class)
    }

    #[must_use]
    pub const fn metrics(&self) -> CollectorMetrics {
        self.metrics
    }
}

impl Default for BootstrapRuntime {
    fn default() -> Self {
        Self::new()
    }
}
