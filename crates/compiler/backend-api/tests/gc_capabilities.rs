use pop_backend_api::{
    BackendCapabilities, BackendGcCapability, RuntimeProfile, RuntimeProfileError,
};
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
        .expect("complete target")
}

#[test]
fn bootstrap_requires_precise_backend_roots_target_stack_maps_and_abi_one() {
    let backend = BackendCapabilities::new([BackendGcCapability::PreciseRoots]);
    let precise_target = target(&[TargetCapability::PreciseStackMaps]);

    assert_eq!(
        backend.validate_runtime_profile(
            RuntimeProfile::BootstrapStableHandles,
            &precise_target,
            1,
        ),
        Ok(())
    );
    assert_eq!(
        BackendCapabilities::default().validate_runtime_profile(
            RuntimeProfile::BootstrapStableHandles,
            &precise_target,
            1,
        ),
        Err(RuntimeProfileError::MissingBackendCapability(
            BackendGcCapability::PreciseRoots,
        ))
    );
    assert_eq!(
        backend.validate_runtime_profile(RuntimeProfile::BootstrapStableHandles, &target(&[]), 1,),
        Err(RuntimeProfileError::MissingTargetCapability(
            TargetCapability::PreciseStackMaps,
        ))
    );
    assert_eq!(
        backend.validate_runtime_profile(
            RuntimeProfile::BootstrapStableHandles,
            &precise_target,
            2,
        ),
        Err(RuntimeProfileError::IncompatibleNativeAbi {
            profile: RuntimeProfile::BootstrapStableHandles,
            major: 2,
        })
    );
}

#[test]
fn production_requires_backend_and_target_relocation_plus_abi_two() {
    let precise_backend = BackendCapabilities::new([BackendGcCapability::PreciseRoots]);
    let relocating_backend = BackendCapabilities::new([
        BackendGcCapability::PreciseRoots,
        BackendGcCapability::RelocatingManagedReferences,
    ]);
    let precise_target = target(&[TargetCapability::PreciseStackMaps]);
    let relocating_target = target(&[
        TargetCapability::PreciseStackMaps,
        TargetCapability::RelocatingNursery,
    ]);

    assert_eq!(
        precise_backend.validate_runtime_profile(
            RuntimeProfile::ProductionGenerational,
            &relocating_target,
            2,
        ),
        Err(RuntimeProfileError::MissingBackendCapability(
            BackendGcCapability::RelocatingManagedReferences,
        ))
    );
    assert_eq!(
        relocating_backend.validate_runtime_profile(
            RuntimeProfile::ProductionGenerational,
            &precise_target,
            2,
        ),
        Err(RuntimeProfileError::MissingTargetCapability(
            TargetCapability::RelocatingNursery,
        ))
    );
    assert_eq!(
        relocating_backend.validate_runtime_profile(
            RuntimeProfile::ProductionGenerational,
            &relocating_target,
            1,
        ),
        Err(RuntimeProfileError::IncompatibleNativeAbi {
            profile: RuntimeProfile::ProductionGenerational,
            major: 1,
        })
    );
    assert_eq!(
        relocating_backend.validate_runtime_profile(
            RuntimeProfile::ProductionGenerational,
            &relocating_target,
            2,
        ),
        Ok(())
    );
}
