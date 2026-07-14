#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlriVersion {
    major: u16,
    minor: u16,
}

impl PlriVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    #[must_use]
    pub const fn major(self) -> u16 {
        self.major
    }

    #[must_use]
    pub const fn minor(self) -> u16 {
        self.minor
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GarbageCollectorContract {
    stage: GarbageCollectorStage,
    roots: RootPrecision,
    nursery: NurseryMobility,
    mature_heap: MatureHeapCollection,
    barriers: BarrierContract,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GarbageCollectorStage {
    BootstrapPreciseStopTheWorld,
    RelocationConformance,
    NativeStableGenerationalConformance,
    ProductionConcurrentGenerational,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootPrecision {
    Precise,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NurseryMobility {
    Absent,
    Moving,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatureHeapCollection {
    Retained,
    StopTheWorldMarkSweep,
    IncrementalSatb,
    MostlyNonMovingConcurrent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BarrierContract {
    None,
    GenerationalCard,
    SatbAndGenerationalCard,
}

impl GarbageCollectorContract {
    #[must_use]
    pub const fn pop_v1() -> Self {
        Self {
            stage: GarbageCollectorStage::ProductionConcurrentGenerational,
            roots: RootPrecision::Precise,
            nursery: NurseryMobility::Moving,
            mature_heap: MatureHeapCollection::MostlyNonMovingConcurrent,
            barriers: BarrierContract::SatbAndGenerationalCard,
        }
    }

    #[must_use]
    pub const fn bootstrap_stage1() -> Self {
        Self {
            stage: GarbageCollectorStage::BootstrapPreciseStopTheWorld,
            roots: RootPrecision::Precise,
            nursery: NurseryMobility::Absent,
            mature_heap: MatureHeapCollection::StopTheWorldMarkSweep,
            barriers: BarrierContract::None,
        }
    }

    #[must_use]
    pub const fn relocation_conformance_stage2() -> Self {
        Self {
            stage: GarbageCollectorStage::RelocationConformance,
            roots: RootPrecision::Precise,
            nursery: NurseryMobility::Moving,
            mature_heap: MatureHeapCollection::Retained,
            barriers: BarrierContract::GenerationalCard,
        }
    }

    #[must_use]
    pub const fn native_stable_generational() -> Self {
        Self {
            stage: GarbageCollectorStage::NativeStableGenerationalConformance,
            roots: RootPrecision::Precise,
            nursery: NurseryMobility::Absent,
            mature_heap: MatureHeapCollection::IncrementalSatb,
            barriers: BarrierContract::SatbAndGenerationalCard,
        }
    }

    #[must_use]
    pub const fn stage(self) -> GarbageCollectorStage {
        self.stage
    }

    #[must_use]
    pub const fn precise_roots(self) -> bool {
        matches!(self.roots, RootPrecision::Precise)
    }

    #[must_use]
    pub const fn moving_nursery(self) -> bool {
        matches!(self.nursery, NurseryMobility::Moving)
    }

    #[must_use]
    pub const fn mostly_non_moving_mature_heap(self) -> bool {
        matches!(
            self.mature_heap,
            MatureHeapCollection::MostlyNonMovingConcurrent
        )
    }

    #[must_use]
    pub const fn concurrent_mature_marking(self) -> bool {
        matches!(
            self.mature_heap,
            MatureHeapCollection::MostlyNonMovingConcurrent
        )
    }

    #[must_use]
    pub const fn satb_barrier(self) -> bool {
        matches!(self.barriers, BarrierContract::SatbAndGenerationalCard)
    }

    #[must_use]
    pub const fn generational_card_barrier(self) -> bool {
        matches!(
            self.barriers,
            BarrierContract::GenerationalCard | BarrierContract::SatbAndGenerationalCard
        )
    }

    #[must_use]
    pub const fn user_finalizers(self) -> bool {
        false
    }

    #[must_use]
    pub const fn weak_references(self) -> bool {
        false
    }

    #[must_use]
    pub const fn conservative_scanning(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorContract {
    typed_results: bool,
    panics_unwind: bool,
    exceptions_are_ordinary_errors: bool,
}

impl ErrorContract {
    #[must_use]
    pub const fn pop_v1() -> Self {
        Self {
            typed_results: true,
            panics_unwind: true,
            exceptions_are_ordinary_errors: false,
        }
    }

    #[must_use]
    pub const fn uses_typed_results(self) -> bool {
        self.typed_results
    }

    #[must_use]
    pub const fn panics_unwind(self) -> bool {
        self.panics_unwind
    }

    #[must_use]
    pub const fn exceptions_are_ordinary_errors(self) -> bool {
        self.exceptions_are_ordinary_errors
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationState {
    Unloaded,
    Loading,
    Loaded,
    Initializing,
    Ready,
    Failed,
}

impl InitializationState {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Unloaded, Self::Loading)
                | (Self::Loading, Self::Loaded | Self::Failed)
                | (Self::Loaded, Self::Initializing)
                | (Self::Initializing, Self::Ready | Self::Failed)
        )
    }
}
