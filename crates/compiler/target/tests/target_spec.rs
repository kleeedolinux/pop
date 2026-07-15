use pop_target::{
    CAbiScalarKind, CAbiSignedness, Endianness, ObjectFormat, OperatingSystem, PointerWidth,
    TargetCapability, TargetSpec,
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
    assert!(target.supports(TargetCapability::Exceptions));
}

#[test]
fn native_target_owns_the_closed_c_scalar_data_model() {
    let target = TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target");

    let character = target
        .c_abi_scalar_layout(CAbiScalarKind::Char)
        .expect("C ABI");
    assert_eq!(character.size(), 1);
    assert_eq!(character.alignment(), 1);
    assert_eq!(character.signedness(), CAbiSignedness::Signed);

    let long = target
        .c_abi_scalar_layout(CAbiScalarKind::Long)
        .expect("C long");
    assert_eq!((long.size(), long.alignment()), (8, 8));
    assert_eq!(long.signedness(), CAbiSignedness::Signed);

    let size = target
        .c_abi_scalar_layout(CAbiScalarKind::Size)
        .expect("size_t");
    assert_eq!((size.size(), size.alignment()), (8, 8));
    assert_eq!(size.signedness(), CAbiSignedness::Unsigned);
    assert_eq!(target.ffi_pointer_layout(), Some((8, 8)));
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
    assert_eq!(little.c_abi_scalar_layout(CAbiScalarKind::Int), None);

    let big = TargetSpec::for_triple("bpfeb-unknown-none").expect("bpfeb target");
    assert_eq!(big.endianness(), Endianness::Big);
    assert!(TargetSpec::for_triple("bpf-unknown-linux").is_err());
}
