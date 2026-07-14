//! Public typed page, placement, configuration, and metric vocabulary.

use pop_runtime_interface::{ObjectSlot, RuntimeTypeId};

use crate::ownership::SchedulerId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub struct AllocationInfrastructureConfig {
    pub(super) page_bytes: usize,
    pub(super) region_bytes: usize,
    pub(super) tlab_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationInfrastructureError {
    ZeroSize,
    UnalignedSize,
    RegionPageMismatch,
    TlabExceedsPage,
}

impl AllocationInfrastructureConfig {
    /// Defines logical page, region, and TLAB geometry.
    ///
    /// # Errors
    ///
    /// Rejects zero, unaligned, non-divisible, or oversized geometry.
    pub const fn new(
        page_bytes: usize,
        region_bytes: usize,
        tlab_bytes: usize,
    ) -> Result<Self, AllocationInfrastructureError> {
        if page_bytes == 0 || region_bytes == 0 || tlab_bytes == 0 {
            return Err(AllocationInfrastructureError::ZeroSize);
        }
        if !page_bytes.is_multiple_of(8)
            || !region_bytes.is_multiple_of(8)
            || !tlab_bytes.is_multiple_of(8)
        {
            return Err(AllocationInfrastructureError::UnalignedSize);
        }
        if !region_bytes.is_multiple_of(page_bytes) {
            return Err(AllocationInfrastructureError::RegionPageMismatch);
        }
        if tlab_bytes > page_bytes {
            return Err(AllocationInfrastructureError::TlabExceedsPage);
        }
        Ok(Self {
            page_bytes,
            region_bytes,
            tlab_bytes,
        })
    }

    #[must_use]
    pub const fn page_bytes(self) -> usize {
        self.page_bytes
    }

    #[must_use]
    pub const fn region_bytes(self) -> usize {
        self.region_bytes
    }

    #[must_use]
    pub const fn tlab_bytes(self) -> usize {
        self.tlab_bytes
    }
}

impl Default for AllocationInfrastructureConfig {
    fn default() -> Self {
        Self::new(32 * 1024, 2 * 1024 * 1024, 16 * 1024)
            .expect("default allocation geometry is valid")
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PageId(pub(super) u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RegionId(pub(super) u64);

impl RegionId {
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HeapDomain {
    LocalEden,
    LocalSurvivor,
    LocalMature,
    Isolated,
    Shared,
    LargeObject,
    Pinned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocationPlacement {
    pub(super) page: PageId,
    pub(super) offset_bytes: usize,
    pub(super) size_bytes: usize,
    pub(super) domain: HeapDomain,
}

impl AllocationPlacement {
    #[must_use]
    pub const fn page(self) -> PageId {
        self.page
    }

    #[must_use]
    pub const fn offset_bytes(self) -> usize {
        self.offset_bytes
    }

    #[must_use]
    pub const fn size_bytes(self) -> usize {
        self.size_bytes
    }

    #[must_use]
    pub const fn domain(self) -> HeapDomain {
        self.domain
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageDescriptor {
    pub(super) id: PageId,
    pub(super) region: RegionId,
    pub(super) domain: HeapDomain,
    pub(super) scheduler: Option<SchedulerId>,
    pub(super) type_id: RuntimeTypeId,
    pub(super) slot_count: u32,
    pub(super) reference_slots: Vec<ObjectSlot>,
    pub(super) capacity_bytes: usize,
}

impl PageDescriptor {
    #[must_use]
    pub const fn id(&self) -> PageId {
        self.id
    }

    #[must_use]
    pub const fn region(&self) -> RegionId {
        self.region
    }

    #[must_use]
    pub const fn domain(&self) -> HeapDomain {
        self.domain
    }

    #[must_use]
    pub const fn scheduler(&self) -> Option<SchedulerId> {
        self.scheduler
    }

    #[must_use]
    pub const fn type_id(&self) -> RuntimeTypeId {
        self.type_id
    }

    #[must_use]
    pub const fn slot_count(&self) -> u32 {
        self.slot_count
    }

    #[must_use]
    pub fn reference_slots(&self) -> &[ObjectSlot] {
        &self.reference_slots
    }

    #[must_use]
    pub fn pointer_free(&self) -> bool {
        self.reference_slots.is_empty()
    }

    #[must_use]
    pub const fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocationMetrics {
    pub(super) tlab_allocations: u64,
    pub(super) tlab_refills: u64,
    pub(super) pages_created: u64,
    pub(super) allocated_bytes: u64,
    pub(super) survivor_copies: u64,
    pub(super) promotions: u64,
    pub(super) pages_returned: u64,
    pub(super) page_reclamation_passes: u64,
    pub(super) mature_page_index_hits: u64,
}

impl AllocationMetrics {
    #[must_use]
    pub const fn tlab_allocations(self) -> u64 {
        self.tlab_allocations
    }

    #[must_use]
    pub const fn tlab_refills(self) -> u64 {
        self.tlab_refills
    }

    #[must_use]
    pub const fn pages_created(self) -> u64 {
        self.pages_created
    }

    #[must_use]
    pub const fn allocated_bytes(self) -> u64 {
        self.allocated_bytes
    }

    #[must_use]
    pub const fn survivor_copies(self) -> u64 {
        self.survivor_copies
    }

    #[must_use]
    pub const fn promotions(self) -> u64 {
        self.promotions
    }

    #[must_use]
    pub const fn pages_returned(self) -> u64 {
        self.pages_returned
    }

    #[must_use]
    pub const fn page_reclamation_passes(self) -> u64 {
        self.page_reclamation_passes
    }

    #[must_use]
    pub const fn mature_page_index_hits(self) -> u64 {
        self.mature_page_index_hits
    }
}
