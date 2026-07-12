#[path = "workload/array.rs"]
mod array;
#[path = "workload/model.rs"]
mod model;
#[path = "workload/pin.rs"]
mod pin;
#[path = "workload/pressure.rs"]
mod pressure;
#[path = "workload/rooted.rs"]
mod rooted;
#[path = "workload/tiny.rs"]
mod tiny;

pub use array::run_managed_array;
pub use model::{WorkloadConfiguration, WorkloadCounters};
pub use pin::run_pin_pressure;
pub use pressure::run_allocation_pressure;
pub use rooted::run_rooted_chain;
pub use tiny::run_tiny_object_churn;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkloadKind {
    TinyObjectChurn,
    RootedChain,
    ManagedArray,
    PinPressure,
    AllocationPressure,
}

impl WorkloadKind {
    pub const ALL: [Self; 5] = [
        Self::TinyObjectChurn,
        Self::RootedChain,
        Self::ManagedArray,
        Self::PinPressure,
        Self::AllocationPressure,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TinyObjectChurn => "tiny_object_churn",
            Self::RootedChain => "rooted_chain",
            Self::ManagedArray => "managed_array",
            Self::PinPressure => "pin_pressure",
            Self::AllocationPressure => "allocation_pressure",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|workload| workload.name() == name)
    }
}

/// Runs one selected deterministic logical workload.
///
/// # Errors
///
/// Returns a portable runtime failure when the configuration or collector
/// operation fails.
pub fn run_workload(
    workload: WorkloadKind,
    configuration: WorkloadConfiguration,
) -> Result<WorkloadCounters, pop_runtime_interface::RuntimeFailure> {
    match workload {
        WorkloadKind::TinyObjectChurn => run_tiny_object_churn(configuration),
        WorkloadKind::RootedChain => run_rooted_chain(configuration),
        WorkloadKind::ManagedArray => run_managed_array(configuration),
        WorkloadKind::PinPressure => run_pin_pressure(configuration),
        WorkloadKind::AllocationPressure => run_allocation_pressure(configuration),
    }
}
