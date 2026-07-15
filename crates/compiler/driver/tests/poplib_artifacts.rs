use std::path::{Path, PathBuf};

use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, NativeLinkPlanSource, PoplibEmission, PoplibError,
    analyze_bubble, emit_poplib, load_poplib, resolve_native_link_inputs,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_projects::{BubbleKind, parse_package_manifest};
use pop_source::SourceFile;
use pop_target::TargetSpec;

const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn module(text: &str) -> FrontEndModule {
    FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(FileId::from_raw(0), "src/lib.pop", text).expect("source"),
    )
}

fn temporary_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pop-poplib-artifact-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove prior artifact fixture");
    }
    std::fs::create_dir_all(&root).expect("create artifact fixture");
    root
}

fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn poplib_emission_round_trips_generic_metadata_and_rejects_corruption() {
    let bubble = BubbleId::from_raw(2);
    let library = analyze_bubble(FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            "namespace Pop.Sequence\n\
             private function privateIdentity<T>(value: T): T\n\
                 return value\n\
             end\n\
             public function identity<T>(value: T): T\n\
                 return privateIdentity(value)\n\
             end\n",
        )],
    ));
    assert!(
        library.diagnostics().is_empty(),
        "{}",
        library.diagnostic_snapshot()
    );
    let root = temporary_root();
    let native_link_plan = parse_package_manifest(
        "[package]\nname = \"Pop.Standard\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nZlib = { kind = \"system\", name = \"z\" }\n",
    )
    .expect("native requirements")
    .native_link_plan("x86_64-unknown-linux-gnu")
    .expect("native link plan");
    let native_link_resolution = resolve_native_link_inputs(
        &[NativeLinkPlanSource::new(&root, native_link_plan.clone())],
        &TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target"),
    )
    .expect("resolved native provider");
    let emission = PoplibEmission::new(
        "Pop.Standard",
        "0.1.0",
        ZERO_SHA256,
        "Pop.Standard",
        BubbleKind::Library,
        "2026",
        library
            .reference_metadata()
            .expect("reference metadata")
            .clone(),
    )
    .with_native_link_plan(native_link_plan)
    .with_resolved_native_providers(native_link_resolution.providers().to_vec())
    .with_documentation(b"<?xml version=\"1.0\"?><doc/>\n".to_vec())
    .with_target_implementation("x86_64-unknown-linux-gnu", b"opaque-native-object".to_vec());
    let artifact = root.join("Pop.Standard.poplib");

    emit_poplib(&artifact, &emission).expect("verified artifact emission");
    let manifest = bytes(&artifact.join("bubble.manifest"));
    let reference = bytes(&artifact.join("reference.metadata"));
    let loaded = load_poplib(&artifact).expect("verified artifact loading");
    assert_eq!(
        loaded.reference_metadata(),
        library.reference_metadata().expect("reference metadata")
    );
    assert_eq!(
        loaded.documentation(),
        Some(b"<?xml version=\"1.0\"?><doc/>\n".as_slice())
    );
    assert_eq!(
        loaded.target_implementation(),
        Some((
            "x86_64-unknown-linux-gnu",
            b"opaque-native-object".as_slice()
        ))
    );
    assert_eq!(loaded.public_api_sha256().len(), 64);
    assert_eq!(loaded.native_link_plans().len(), 1);
    assert_eq!(loaded.native_link_plans()[0].libraries()[0].alias(), "Zlib");
    assert_eq!(loaded.resolved_native_providers().len(), 1);
    assert_eq!(loaded.resolved_native_providers()[0].identity(), "z");
    assert_eq!(loaded.resolved_native_providers()[0].version(), None);
    assert_eq!(
        loaded.resolved_native_providers()[0].link_libraries(),
        ["z"]
    );

    emit_poplib(&artifact, &emission).expect("deterministic replacement");
    assert_eq!(bytes(&artifact.join("bubble.manifest")), manifest);
    assert_eq!(bytes(&artifact.join("reference.metadata")), reference);

    std::fs::write(
        artifact.join("targets/x86_64-unknown-linux-gnu/native.object"),
        b"corrupt",
    )
    .expect("corrupt target");
    assert_eq!(load_poplib(&artifact), Err(PoplibError::SizeMismatch));

    emit_poplib(&artifact, &emission).expect("restore artifact");
    std::fs::write(artifact.join("unexpected"), b"extra").expect("extra file");
    assert_eq!(load_poplib(&artifact), Err(PoplibError::UnexpectedFile));

    std::fs::remove_dir_all(root).expect("remove artifact fixture");
}
