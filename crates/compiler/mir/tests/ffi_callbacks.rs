use std::fs;
use std::path::PathBuf;

use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, VerifiedFfiGeneratedBindings, analyze_bubble,
    artifact_sha256_hex, generate_ffi_bindings, verify_ffi_generated_bindings,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{
    MirVerificationError, lower_hir_bubble_with_fingerprint, parse_mir_dump, verify_mir_bubble,
};
use pop_projects::parse_package_manifest;
use pop_source::SourceFile;

fn generated_callback_bindings() -> (PathBuf, SourceFile, Vec<VerifiedFfiGeneratedBindings>) {
    let descriptor = include_str!("../../backends/llvm/tests/fixtures/ffi_callbacks.popc");
    let root = std::env::temp_dir().join(format!(
        "pop-mir-callback-text-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    if root.exists() {
        fs::remove_dir_all(&root).expect("remove prior callback text fixture");
    }
    fs::create_dir_all(root.join("native")).expect("create callback descriptor directory");
    fs::write(root.join("native/callbacks.popc"), descriptor).expect("write callback descriptor");
    let manifest_text = format!(
        "[package]\nname = \"Callback.Fixture\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[platform.\"x86_64-unknown-linux-gnu\".ffiGenerators]\nCallbacks = {{ descriptor = \"native/callbacks.popc\", descriptorSha256 = \"{}\", outputDirectory = \"src/generated/callbacks\" }}\n",
        artifact_sha256_hex(descriptor.as_bytes())
    );
    let manifest_path = root.join("bubble.toml");
    fs::write(&manifest_path, &manifest_text).expect("write callback manifest");
    generate_ffi_bindings(&manifest_path, "x86_64-unknown-linux-gnu", "Callbacks")
        .expect("generate callback bindings");
    let manifest = parse_package_manifest(&manifest_text).expect("parse callback manifest");
    let verified = verify_ffi_generated_bindings(&root, &manifest, "x86_64-unknown-linux-gnu")
        .expect("verify generated callback bindings");
    let source_path = "src/generated/callbacks/bindings.pop";
    let source_text = fs::read_to_string(root.join(source_path)).expect("read callback source");
    let source = SourceFile::new(FileId::from_raw(0), source_path, source_text)
        .expect("generated callback source");
    (root, source, verified)
}

#[test]
fn callback_operations_round_trip_every_exact_typed_contract_fact() {
    let ffi = BubbleId::from_raw(20);
    let (fixture_root, generated, verified) = generated_callback_bindings();
    let source = SourceFile::new(
        FileId::from_raw(1),
        "src/callbacks.pop",
        include_str!("../../backends/llvm/tests/fixtures/ffi_callbacks.pop"),
    )
    .expect("callback source");
    let front_end = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![
                FrontEndModule::new(ModuleId::from_raw(0), generated),
                FrontEndModule::new(ModuleId::from_raw(1), source),
            ],
        )
        .with_ffi_dependency(ffi)
        .with_verified_ffi_generated_bindings(verified),
    );
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble_with_fingerprint(
        front_end.hir().expect("callback HIR"),
        front_end.types(),
        artifact_sha256_hex,
    )
    .expect("verified callback MIR");

    let dump = mir.dump();
    for exact_fact in [
        "ffiCallbackOpenScoped",
        "ffiCallbackOpenOwned",
        "callCallbackPair",
        "ffiCallbackCloseScoped",
        "ffiCallbackCloseOwned",
        "callbackType t",
        "owner s",
        "function nf",
        "callbackSite#",
        "thread AttachedThread",
        "abi C",
        "abi System",
        "parameterLayouts[",
        "resultLayout ",
        "fingerprint b95f44146facefea58f2e5d6153fef8e19173a14436501921a3381285e105788",
        "fingerprint 2beb2c45d821699608324693b75c59ed238fcaa80ea31b1d7de18a69317fa8c5",
        "lifetime CallScoped",
        "lifetime Registered",
        "callbackPairs(",
        "captures[",
        "region#",
    ] {
        assert!(dump.contains(exact_fact), "missing {exact_fact}:\n{dump}");
    }
    for (operation, count) in [
        ("ffiCallbackOpenScoped", 4),
        ("ffiCallbackOpenOwned", 1),
        ("callCallbackPair", 6),
        ("ffiCallbackCloseScoped", 4),
        ("ffiCallbackCloseOwned", 1),
    ] {
        assert_eq!(
            dump.matches(operation).count(),
            count,
            "{operation}:\n{dump}"
        );
    }
    let reparsed = parse_mir_dump(&dump)
        .expect("callback MIR text round trip")
        .with_ffi_layouts(mir.ffi_layouts().clone());
    assert_eq!(reparsed.dump(), dump);
    assert_eq!(
        verify_mir_bubble(&reparsed, front_end.types()),
        Ok(()),
        "{dump}"
    );

    let forged = dump.replacen(
        "fingerprint b95f44146facefea58f2e5d6153fef8e19173a14436501921a3381285e105788 owner",
        "fingerprint 095f44146facefea58f2e5d6153fef8e19173a14436501921a3381285e105788 owner",
        1,
    );
    let forged = parse_mir_dump(&forged)
        .expect("structurally valid forged callback MIR")
        .with_ffi_layouts(mir.ffi_layouts().clone());
    assert!(
        verify_mir_bubble(&forged, front_end.types()).is_err_and(|errors| errors.iter().any(
            |error| matches!(
                error,
                MirVerificationError::InvalidFfiCallbackOperation { .. }
            )
        ))
    );

    fs::remove_dir_all(fixture_root).expect("remove callback text fixture");
}
