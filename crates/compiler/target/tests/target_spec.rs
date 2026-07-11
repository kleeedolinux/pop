use pop_target::{Endianness, PointerWidth, TargetCapability, TargetSpec};

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
