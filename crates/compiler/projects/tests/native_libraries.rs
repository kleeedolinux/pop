use pop_projects::{
    ManifestError, NativeLibraryDiscovery, NativeLibraryKind, NativeLinkPlan, NativeLinkPlanError,
    parse_package_manifest, sha256_hex,
};

const ARCHIVE_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SHARED_HASH: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

#[test]
fn adr_0081_native_link_plan_is_typed_targeted_and_sorted() {
    let manifest = parse_package_manifest(&format!(
        "[package]\n\
         name = \"Example.Bindings\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [nativeLibraries]\n\
         Zlib = {{ kind = \"system\", name = \"z\" }}\n\
         Pcre = {{ kind = \"system\", name = \"libpcre2-8\", discovery = \"packageConfiguration\", version = \"10.42\" }}\n\
         Codec = {{ kind = \"archive\", path = \"native/libcodec.a\", sha256 = \"{ARCHIVE_HASH}\" }}\n\
         [platform.\"x86_64-unknown-linux-gnu\".nativeLibraries]\n\
         PlatformCodec = {{ kind = \"shared\", path = \"native/libplatformCodec.so\", sha256 = \"{SHARED_HASH}\" }}\n"
    ))
    .expect("ADR 0081 manifest");

    assert_eq!(
        manifest
            .native_libraries()
            .iter()
            .map(pop_projects::NativeLibrary::alias)
            .collect::<Vec<_>>(),
        ["Codec", "Pcre", "Zlib"]
    );

    let plan = manifest
        .native_link_plan("x86_64-unknown-linux-gnu")
        .expect("target plan");
    assert_eq!(plan.platform_target(), "x86_64-unknown-linux-gnu");
    assert_eq!(
        plan.libraries()
            .iter()
            .map(|library| (library.alias(), library.kind()))
            .collect::<Vec<_>>(),
        [
            ("Codec", NativeLibraryKind::Archive),
            ("Pcre", NativeLibraryKind::System),
            ("PlatformCodec", NativeLibraryKind::Shared),
            ("Zlib", NativeLibraryKind::System),
        ]
    );

    let pcre = plan
        .libraries()
        .iter()
        .find(|library| library.alias() == "Pcre")
        .expect("Pcre entry");
    assert_eq!(pcre.name(), Some("libpcre2-8"));
    assert_eq!(
        pcre.discovery(),
        Some(NativeLibraryDiscovery::PackageConfiguration)
    );
    assert_eq!(pcre.version_requirement(), Some("10.42"));
    assert_eq!(pcre.path(), None);

    let codec = plan
        .libraries()
        .iter()
        .find(|library| library.alias() == "Codec")
        .expect("Codec entry");
    assert_eq!(codec.path(), Some("native/libcodec.a"));
    assert_eq!(codec.sha256(), Some(ARCHIVE_HASH));
    assert_eq!(codec.name(), None);

    assert!(
        manifest
            .native_link_plan("aarch64-apple-darwin")
            .expect("other target plan")
            .libraries()
            .iter()
            .all(|library| library.alias() != "PlatformCodec")
    );
}

#[test]
fn adr_0081_default_c_environment_needs_no_native_library_entry() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Example.Libc\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n",
    )
    .expect("manifest without native inputs");

    assert!(manifest.native_libraries().is_empty());
    assert!(
        manifest
            .native_link_plan("x86_64-unknown-linux-gnu")
            .expect("empty explicit plan")
            .libraries()
            .is_empty()
    );
}

#[test]
fn adr_0093_ffi_generators_are_exact_target_owned_and_sorted() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Example.Bindings\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [nativeLibraries]\n\
         Zlib = { kind = \"system\", name = \"z\" }\n\
         [platform.\"x86_64-unknown-linux-gnu\".ffiGenerators]\n\
         Zlib = { nativeLibrary = \"Zlib\", descriptor = \"native/zlib.popc\", descriptorSha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\", outputDirectory = \"src/generated/zlib\" }\n\
         C = { descriptor = \"native/c.popc\", descriptorSha256 = \"abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789\", outputDirectory = \"src/generated/c\" }\n",
    )
    .expect("ADR 0093 manifest");

    assert_eq!(manifest.platform_ffi_generators().len(), 1);
    assert_eq!(
        manifest.platform_ffi_generators()[0]
            .generators()
            .iter()
            .map(pop_projects::FfiGenerator::alias)
            .collect::<Vec<_>>(),
        ["C", "Zlib"]
    );
    let zlib = manifest
        .ffi_generator("x86_64-unknown-linux-gnu", "Zlib")
        .expect("exact target generator");
    assert_eq!(zlib.native_library(), Some("Zlib"));
    assert_eq!(zlib.descriptor(), "native/zlib.popc");
    assert_eq!(zlib.output_directory(), "src/generated/zlib");
    assert_eq!(
        manifest.ffi_generator("aarch64-unknown-linux-gnu", "Zlib"),
        Err(ManifestError::MissingFfiGenerator)
    );
}

#[test]
fn adr_0093_ffi_generator_manifest_rejects_unsafe_or_untyped_inputs() {
    let cases = [
        "Zlib = { descriptor = \"../zlib.popc\", descriptorSha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\", outputDirectory = \"src/generated/zlib\" }",
        "Zlib = { descriptor = \"native/zlib.json\", descriptorSha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\", outputDirectory = \"src/generated/zlib\" }",
        "Zlib = { descriptor = \"native/zlib.popc\", descriptorSha256 = \"bad\", outputDirectory = \"src/generated/zlib\" }",
        "Zlib = { descriptor = \"native/zlib.popc\", descriptorSha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\", outputDirectory = \"generated/zlib\" }",
        "Zlib = { descriptor = \"native/zlib`command`.popc\", descriptorSha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\", outputDirectory = \"src/generated/zlib\" }",
        "Zlib = { descriptor = \"native/zlib.popc\", descriptorSha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\", outputDirectory = \"src/generated/zlib\", header = \"zlib.h\" }",
    ];
    for entry in cases {
        let manifest = format!(
            "[package]\nname = \"Example.Bindings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[platform.\"x86_64-unknown-linux-gnu\".ffiGenerators]\n{entry}\n"
        );
        assert!(parse_package_manifest(&manifest).is_err(), "{entry}");
    }
}

#[test]
fn adr_0081_native_inputs_reject_paths_hashes_raw_flags_and_shell_text() {
    let cases = [
        (
            "Codec = { kind = \"archive\", path = \"../libcodec.a\", sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\" }",
            ManifestError::InvalidNativeLibraryPath,
        ),
        (
            "Codec = { kind = \"archive\", path = \"/usr/lib/libcodec.a\", sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\" }",
            ManifestError::InvalidNativeLibraryPath,
        ),
        (
            "Codec = { kind = \"archive\", path = \"@objects.rsp\", sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\" }",
            ManifestError::InvalidNativeLibraryPath,
        ),
        (
            "Codec = { kind = \"archive\", path = \"native/libcodec.a\" }",
            ManifestError::InvalidNativeLibraryHash,
        ),
        (
            "Codec = { kind = \"archive\", path = \"native/libcodec.a\", sha256 = \"ABCDEF\" }",
            ManifestError::InvalidNativeLibraryHash,
        ),
        (
            "Pcre = { kind = \"system\", name = \"pcre\", ldflags = \"-lpcre\" }",
            ManifestError::InvalidNativeLibrary,
        ),
        (
            "Pcre = { kind = \"system\", name = \"`pkg-config pcre --libs`\" }",
            ManifestError::InvalidNativeLibraryName,
        ),
        (
            "Pcre = { kind = \"system\", name = \"pcre\", command = \"pkg-config pcre\" }",
            ManifestError::InvalidNativeLibrary,
        ),
    ];

    for (entry, expected) in cases {
        let text = format!(
            "[package]\nname = \"Example.Bindings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\n{entry}\n"
        );
        assert_eq!(parse_package_manifest(&text), Err(expected), "{entry}");
    }
}

#[test]
fn adr_0081_native_input_shapes_and_target_conflicts_fail_closed() {
    let malformed = [
        (
            "Lowercase = { kind = \"framework\", name = \"Cocoa\", sha256 = \"unused\" }",
            ManifestError::InvalidNativeLibrary,
        ),
        (
            "Wrong = { kind = \"shared\", name = \"codec\" }",
            ManifestError::InvalidNativeLibraryPath,
        ),
        (
            "Wrong = { kind = \"system\", path = \"native/libcodec.so\", sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\" }",
            ManifestError::InvalidNativeLibraryName,
        ),
        (
            "Wrong = { kind = \"object\", path = \"native/codec.o\", sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\", discovery = \"packageConfiguration\" }",
            ManifestError::InvalidNativeLibrary,
        ),
        (
            "Wrong = { kind = \"unknown\", name = \"codec\" }",
            ManifestError::InvalidNativeLibrary,
        ),
    ];
    for (entry, expected) in malformed {
        let text = format!(
            "[package]\nname = \"Example.Bindings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\n{entry}\n"
        );
        assert_eq!(parse_package_manifest(&text), Err(expected), "{entry}");
    }

    let duplicate_for_target = "[package]\n\
        name = \"Example.Bindings\"\n\
        version = \"0.1.0\"\n\
        edition = \"2026\"\n\
        [nativeLibraries]\n\
        Codec = { kind = \"system\", name = \"codec\" }\n\
        [platform.\"x86_64-unknown-linux-gnu\".nativeLibraries]\n\
        Codec = { kind = \"system\", name = \"platformCodec\" }\n";
    let manifest = parse_package_manifest(duplicate_for_target).expect("sections parse alone");
    assert_eq!(
        manifest.native_link_plan("x86_64-unknown-linux-gnu"),
        Err(ManifestError::DuplicateNativeLibrary)
    );
}

#[test]
fn native_link_plan_verifies_exact_regular_local_inputs() {
    let root = std::env::temp_dir().join(format!(
        "pop-native-link-inputs-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove prior native-link fixture");
    }
    std::fs::create_dir_all(root.join("native")).expect("create native-link fixture");
    let bytes = b"deterministic native archive";
    std::fs::write(root.join("native/libcodec.a"), bytes).expect("write native input");
    let manifest = parse_package_manifest(&format!(
        "[package]\nname = \"Example.Bindings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nCodec = {{ kind = \"archive\", path = \"native/libcodec.a\", sha256 = \"{}\" }}\n",
        sha256_hex(bytes)
    ))
    .expect("hashed native input");
    let plan = manifest
        .native_link_plan("x86_64-unknown-linux-gnu")
        .expect("native link plan");

    plan.verify_local_inputs(&root)
        .expect("exact regular input verifies");
    std::fs::write(root.join("native/libcodec.a"), b"changed").expect("mutate native input");
    assert_eq!(
        plan.verify_local_inputs(&root),
        Err(NativeLinkPlanError::HashMismatch)
    );

    std::fs::remove_dir_all(root).expect("remove native-link fixture");
}

#[cfg(unix)]
#[test]
fn native_link_plan_rejects_symlinked_inputs() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!(
        "pop-native-link-symlink-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove prior symlink fixture");
    }
    std::fs::create_dir_all(root.join("native")).expect("create symlink fixture");
    let bytes = b"external native archive";
    std::fs::write(root.join("external.a"), bytes).expect("write external input");
    symlink(root.join("external.a"), root.join("native/libcodec.a")).expect("link native input");
    let manifest = parse_package_manifest(&format!(
        "[package]\nname = \"Example.Bindings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nCodec = {{ kind = \"archive\", path = \"native/libcodec.a\", sha256 = \"{}\" }}\n",
        sha256_hex(bytes)
    ))
    .expect("symlinked native input manifest");
    let plan = manifest
        .native_link_plan("x86_64-unknown-linux-gnu")
        .expect("native link plan");

    assert_eq!(
        plan.verify_local_inputs(&root),
        Err(NativeLinkPlanError::SymlinkInput)
    );
    std::fs::remove_dir_all(root).expect("remove symlink fixture");
}

#[test]
fn native_link_plan_merge_is_sorted_and_rejects_conflicting_aliases() {
    let left = parse_package_manifest(
        "[package]\nname = \"Example.Left\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nCodec = { kind = \"system\", name = \"codec\" }\n",
    )
    .expect("left manifest")
    .native_link_plan("x86_64-unknown-linux-gnu")
    .expect("left plan");
    let same = left.clone();
    let right = parse_package_manifest(
        "[package]\nname = \"Example.Right\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nZlib = { kind = \"system\", name = \"z\" }\n",
    )
    .expect("right manifest")
    .native_link_plan("x86_64-unknown-linux-gnu")
    .expect("right plan");
    let conflict = parse_package_manifest(
        "[package]\nname = \"Example.Conflict\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nCodec = { kind = \"system\", name = \"otherCodec\" }\n",
    )
    .expect("conflicting manifest")
    .native_link_plan("x86_64-unknown-linux-gnu")
    .expect("conflicting plan");

    let merged = NativeLinkPlan::merge(&[right, same, left.clone()]).expect("canonical merge");
    assert_eq!(
        merged
            .libraries()
            .iter()
            .map(pop_projects::NativeLibrary::alias)
            .collect::<Vec<_>>(),
        ["Codec", "Zlib"]
    );
    assert_eq!(
        NativeLinkPlan::merge(&[left, conflict]),
        Err(NativeLinkPlanError::ConflictingAlias)
    );
}
