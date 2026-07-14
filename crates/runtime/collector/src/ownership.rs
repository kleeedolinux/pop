//! Typed object ownership metadata kept distinct from placement and generation.

use pop_runtime_interface::SchedulerId;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IsolatedRegionId(u64);

impl IsolatedRegionId {
    pub(crate) const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IsolationStatistics {
    region: IsolatedRegionId,
    objects_isolated: u64,
}

impl IsolationStatistics {
    pub(crate) fn new(region: IsolatedRegionId, objects_isolated: usize) -> Self {
        Self {
            region,
            objects_isolated: u64::try_from(objects_isolated).unwrap_or(u64::MAX),
        }
    }

    #[must_use]
    pub const fn region(self) -> IsolatedRegionId {
        self.region
    }

    #[must_use]
    pub const fn objects_isolated(self) -> u64 {
        self.objects_isolated
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IsolationTelemetry {
    pub(crate) regions_created: u64,
    pub(crate) transfers_completed: u64,
    pub(crate) objects_transferred: u64,
    pub(crate) regions_dissolved: u64,
}

impl IsolationTelemetry {
    #[must_use]
    pub const fn regions_created(self) -> u64 {
        self.regions_created
    }

    #[must_use]
    pub const fn transfers_completed(self) -> u64 {
        self.transfers_completed
    }

    #[must_use]
    pub const fn objects_transferred(self) -> u64 {
        self.objects_transferred
    }

    #[must_use]
    pub const fn regions_dissolved(self) -> u64 {
        self.regions_dissolved
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectOwnership {
    SchedulerLocal(SchedulerId),
    Isolated(IsolatedRegionId),
    Shared,
}

impl Default for ObjectOwnership {
    fn default() -> Self {
        Self::SchedulerLocal(SchedulerId::new(1))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationStatistics {
    objects_published: u64,
}

impl PublicationStatistics {
    pub(crate) fn new(objects_published: usize) -> Self {
        Self {
            objects_published: u64::try_from(objects_published).unwrap_or(u64::MAX),
        }
    }

    #[must_use]
    pub const fn objects_published(self) -> u64 {
        self.objects_published
    }
}
