//! Typed object ownership metadata kept distinct from placement and generation.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchedulerId(u32);

impl SchedulerId {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IsolatedRegionId(u64);

impl IsolatedRegionId {
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
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
