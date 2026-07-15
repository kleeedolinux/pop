use pop_target::{
    Endianness, ObjectFormat, OperatingSystem, PointerWidth, TargetCapability, TargetSpec,
};

#[test]
fn target_spec_exposes_backend_neutral_facts() {
    let target = TargetSpec::builder("x86_64-unknown-linux-gnu")
        .pointer_width(PointerWidth::Bits64)
        .endianness(Endianness::Little)
        .capability(TargetCapability::Threads)
        .capability(TargetCapability::PreciseStackMaps)
        .build()
        .expect("complete target");

    assert_eq!(target.triple(), "x86_64-unknown-linux-gnu");
    assert_eq!(target.pointer_width(), PointerWidth::Bits64);
    assert!(target.supports(TargetCapability::Threads));
    assert!(target.supports(TargetCapability::PreciseStackMaps));
    assert!(!target.supports(TargetCapability::Simd));
    assert!(!format!("{target:?}").to_ascii_lowercase().contains("llvm"));
}

#[test]
fn native_target_declares_relocating_nursery_feasibility() {
    let target = TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target");

    assert!(target.supports(TargetCapability::PreciseStackMaps));
    assert!(target.supports(TargetCapability::RelocatingNursery));
}

#[test]
fn bpf_target_specs_are_elf_llvm_bpf_targets() {
    let little = TargetSpec::for_triple("bpfel-unknown-none").expect("bpfel target");
    assert_eq!(little.pointer_width(), PointerWidth::Bits64);
    assert_eq!(little.endianness(), Endianness::Little);
    assert_eq!(little.object_format(), ObjectFormat::Elf);
    assert_eq!(little.operating_system(), OperatingSystem::None);
    assert!(little.supports(TargetCapability::LlvmBpf));
    assert!(!little.supports(TargetCapability::Threads));
    assert!(!little.supports(TargetCapability::SharedLibraries));

    let big = TargetSpec::for_triple("bpfeb-unknown-none").expect("bpfeb target");
    assert_eq!(big.endianness(), Endianness::Big);
    assert!(TargetSpec::for_triple("bpf-unknown-linux").is_err());
}
