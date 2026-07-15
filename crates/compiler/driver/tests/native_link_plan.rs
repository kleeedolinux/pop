use std::path::PathBuf;

use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, NativeLinkInput, NativeLinkPlanSource,
    NativeLinkResolutionError, analyze_bubble, resolve_native_link_inputs,
    validate_foreign_link_aliases,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::lower_hir_bubble;
use pop_projects::{parse_package_manifest, sha256_hex};
use pop_source::SourceFile;
use pop_target::TargetSpec;

fn temporary_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pop-native-link-driver-{}-{name}",
        std::process::id()
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove prior native-link fixture");
    }
    std::fs::create_dir_all(root.join("native")).expect("create native-link fixture");
    root
}

#[test]
fn driver_resolves_only_typed_verified_native_link_inputs() {
    let root = temporary_root("typed-inputs");
    let archive = b"verified archive";
    std::fs::write(root.join("native/libanswer.a"), archive).expect("write native archive");
    let manifest = parse_package_manifest(&format!(
        "[package]\nname = \"Example.Native\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nAnswer = {{ kind = \"archive\", path = \"native/libanswer.a\", sha256 = \"{}\" }}\nSystemC = {{ kind = \"system\", name = \"c\" }}\n",
        sha256_hex(archive)
    ))
    .expect("native manifest");
    let plan = manifest
        .native_link_plan("x86_64-unknown-linux-gnu")
        .expect("native link plan");
    let inputs = resolve_native_link_inputs(
        &[NativeLinkPlanSource::new(&root, plan)],
        &TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target"),
    )
    .expect("typed native inputs");

    assert_eq!(
        inputs,
        [
            NativeLinkInput::File(root.join("native/libanswer.a")),
            NativeLinkInput::SystemLibrary("c".to_owned()),
        ]
    );
    std::fs::remove_dir_all(root).expect("remove native-link fixture");
}

#[test]
fn driver_rejects_frameworks_on_an_incompatible_target() {
    let root = temporary_root("framework");
    let manifest = parse_package_manifest(
        "[package]\nname = \"Example.Native\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nCocoa = { kind = \"framework\", name = \"Cocoa\" }\n",
    )
    .expect("framework manifest");
    let plan = manifest
        .native_link_plan("x86_64-unknown-linux-gnu")
        .expect("framework plan");

    assert_eq!(
        resolve_native_link_inputs(
            &[NativeLinkPlanSource::new(&root, plan)],
            &TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target"),
        ),
        Err(NativeLinkResolutionError::UnsupportedProvider)
    );
    std::fs::remove_dir_all(root).expect("remove native-link fixture");
}

#[test]
fn driver_rejects_host_package_configuration_for_a_non_host_target() {
    let root = temporary_root("cross-package-configuration");
    let manifest = parse_package_manifest(
        "[package]\nname = \"Example.Native\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nMissing = { kind = \"system\", name = \"pop-ffi-test-missing\", discovery = \"packageConfiguration\", version = \"1.0\" }\n",
    )
    .expect("package-configuration manifest");
    let plan = manifest
        .native_link_plan("bpfel-unknown-none")
        .expect("non-host native link plan");

    assert_eq!(
        resolve_native_link_inputs(
            &[NativeLinkPlanSource::new(&root, plan)],
            &TargetSpec::for_triple("bpfel-unknown-none").expect("BPF target"),
        ),
        Err(NativeLinkResolutionError::UnsupportedProvider)
    );
    std::fs::remove_dir_all(root).expect("remove native-link fixture");
}

#[test]
fn every_foreign_link_alias_must_resolve_in_the_package_plan() {
    let ffi = BubbleId::from_raw(9);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/native.pop",
        "@Ffi.Link(\"Answer\")\n\
         namespace Native\n\
         @Ffi.Foreign(\"native_answer\")\n\
         internal function answer(): Ffi.C.Int\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            vec![ffi],
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        )
        .with_ffi_dependency(ffi),
    );
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let missing = parse_package_manifest(
        "[package]\nname = \"Example.Native\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .expect("empty manifest")
    .native_link_plan("x86_64-unknown-linux-gnu")
    .expect("empty plan");
    let present = parse_package_manifest(
        "[package]\nname = \"Example.Native\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nAnswer = { kind = \"system\", name = \"answer\" }\n",
    )
    .expect("linked manifest")
    .native_link_plan("x86_64-unknown-linux-gnu")
    .expect("linked plan");

    assert_eq!(
        validate_foreign_link_aliases(&mir, &missing),
        Err(NativeLinkResolutionError::MissingAlias)
    );
    validate_foreign_link_aliases(&mir, &present).expect("foreign alias resolves");
}
