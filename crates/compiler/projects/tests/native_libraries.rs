use pop_projects::{
    ManifestError, NativeLibraryDiscovery, NativeLibraryKind, parse_package_manifest,
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
