use pop_backend_api::{
    ProgramRequirements, RequirementOrigin, RuntimeContract, RuntimeContractError, RuntimeProfile,
    RuntimeProfileSelectionError, validate_runtime_contracts,
};
use pop_foundation::{FunctionId, ValueId};
use pop_target::{TargetCapability, TargetSpec};

fn bpf_target() -> TargetSpec {
    TargetSpec::for_triple("bpfel-unknown-none").expect("BPF target")
}

fn native_target() -> TargetSpec {
    TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target")
}

#[test]
fn linux_ebpf_profile_satisfies_minimal_scalar_contracts() {
    let mut requirements = ProgramRequirements::default();
    requirements.require_runtime(
        RuntimeContract::IntegerOperations,
        RequirementOrigin::Instruction {
            function: FunctionId::from_raw(0),
            value: ValueId::from_raw(0),
        },
    );
    requirements.require_runtime(
        RuntimeContract::DirectCalls,
        RequirementOrigin::Instruction {
            function: FunctionId::from_raw(0),
            value: ValueId::from_raw(1),
        },
    );

    assert_eq!(
        validate_runtime_contracts(&requirements, RuntimeProfile::LinuxEbpf, &bpf_target()),
        Ok(())
    );
}

#[test]
fn missing_allocator_contract_reports_profile_contract_origin_and_target() {
    let mut requirements = ProgramRequirements::default();
    let origin = RequirementOrigin::Instruction {
        function: FunctionId::from_raw(7),
        value: ValueId::from_raw(11),
    };
    requirements.require_runtime(RuntimeContract::ManagedAllocator, origin);

    let error = validate_runtime_contracts(&requirements, RuntimeProfile::LinuxEbpf, &bpf_target())
        .expect_err("linux-ebpf does not provide allocation");

    assert!(matches!(
        error,
        RuntimeContractError::MissingContract {
            profile: RuntimeProfile::LinuxEbpf,
            ref requirement,
            ..
        } if requirement.contract() == RuntimeContract::ManagedAllocator
            && requirement.origin() == origin
    ));
    let text = error.to_string();
    assert!(text.contains("linux-ebpf"));
    assert!(text.contains("ManagedAllocator"));
    assert!(text.contains("bpfel-unknown-none"));
}

#[test]
fn full_runtime_profile_satisfies_allocator_contract_in_unit_resolution() {
    let mut requirements = ProgramRequirements::default();
    requirements.require_runtime(
        RuntimeContract::ManagedAllocator,
        RequirementOrigin::Instruction {
            function: FunctionId::from_raw(1),
            value: ValueId::from_raw(2),
        },
    );

    assert_eq!(
        validate_runtime_contracts(
            &requirements,
            RuntimeProfile::BootstrapStableHandles,
            &native_target(),
        ),
        Ok(())
    );
}

#[test]
fn runtime_profile_names_are_explicit_and_checked_against_targets() {
    assert_eq!(
        RuntimeProfile::parse("linux-ebpf"),
        Ok(RuntimeProfile::LinuxEbpf)
    );
    assert_eq!(
        RuntimeProfile::parse("not-a-profile"),
        Err(RuntimeProfileSelectionError::UnknownRuntimeProfile(
            "not-a-profile".to_owned()
        ))
    );

    let requirements = ProgramRequirements::default();
    assert!(matches!(
        validate_runtime_contracts(&requirements, RuntimeProfile::LinuxEbpf, &native_target()),
        Err(RuntimeContractError::IncompatibleTarget {
            profile: RuntimeProfile::LinuxEbpf,
            ..
        })
    ));
}

#[test]
fn legacy_gc_profile_validation_accepts_ebpf_profile_without_gc_contracts() {
    let backend = pop_backend_api::BackendCapabilities::default();
    assert_eq!(
        backend.validate_runtime_profile(RuntimeProfile::LinuxEbpf, &bpf_target(), 0),
        Ok(())
    );
    assert_eq!(
        backend.validate_runtime_profile(
            RuntimeProfile::LinuxEbpf,
            &TargetSpec::builder("custom")
                .pointer_width(pop_target::PointerWidth::Bits64)
                .endianness(pop_target::Endianness::Little)
                .build()
                .expect("target"),
            0,
        ),
        Err(
            pop_backend_api::RuntimeProfileError::MissingTargetCapability(
                TargetCapability::LlvmBpf,
            )
        )
    );
}
