use pop_backend_api::{BackendGcCapability, RuntimeProfile, RuntimeProfileError};
use pop_backend_llvm::{
    LlvmLoweringError, LlvmLoweringOptions, llvm_backend_capabilities, lower_mir_to_llvm_ir,
    validate_llvm_runtime_profile,
};
use pop_mir::parse_mir_dump;
use pop_runtime_native_abi::{NATIVE_ABI_1_VERSION, NATIVE_ABI_2_VERSION};
use pop_target::{Endianness, PointerWidth, TargetCapability, TargetSpec};

fn target(capabilities: &[TargetCapability]) -> TargetSpec {
    capabilities
        .iter()
        .copied()
        .fold(
            TargetSpec::builder("x86_64-unknown-linux-gnu")
                .pointer_width(PointerWidth::Bits64)
                .endianness(Endianness::Little),
            pop_target::TargetSpecBuilder::capability,
        )
        .build()
        .expect("target")
}

#[test]
fn llvm_advertises_only_its_verified_writable_root_capabilities() {
    let capabilities = llvm_backend_capabilities();

    assert!(capabilities.supports(BackendGcCapability::PreciseRoots));
    assert!(capabilities.supports(BackendGcCapability::RelocatingManagedReferences));
}

#[test]
fn llvm_profile_negotiation_requires_target_and_exact_native_abi() {
    let production_target = target(&[
        TargetCapability::PreciseStackMaps,
        TargetCapability::RelocatingNursery,
    ]);
    assert_eq!(
        validate_llvm_runtime_profile(
            RuntimeProfile::ProductionGenerational,
            &production_target,
            NATIVE_ABI_2_VERSION,
        ),
        Ok(())
    );
    assert_eq!(
        validate_llvm_runtime_profile(
            RuntimeProfile::ProductionGenerational,
            &target(&[TargetCapability::PreciseStackMaps]),
            NATIVE_ABI_2_VERSION,
        ),
        Err(RuntimeProfileError::MissingTargetCapability(
            TargetCapability::RelocatingNursery,
        ))
    );
    assert_eq!(
        validate_llvm_runtime_profile(
            RuntimeProfile::ProductionGenerational,
            &production_target,
            NATIVE_ABI_1_VERSION,
        ),
        Err(RuntimeProfileError::IncompatibleNativeAbi {
            profile: RuntimeProfile::ProductionGenerational,
            major: 1,
        })
    );
}

#[test]
fn llvm_rejects_an_unavailable_production_profile_before_emission() {
    let mir = parse_mir_dump(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[]\n  b0():\n    return ()\n",
    )
    .expect("MIR");
    let types = pop_types::TypeArena::new();

    assert!(matches!(
        lower_mir_to_llvm_ir(
            &mir,
            &types,
            &target(&[TargetCapability::PreciseStackMaps]),
            LlvmLoweringOptions::default()
                .with_runtime_profile(RuntimeProfile::ProductionGenerational),
        ),
        Err(LlvmLoweringError::RuntimeProfile(
            RuntimeProfileError::MissingTargetCapability(TargetCapability::RelocatingNursery)
        ))
    ));
}
