use pop_projects::{
    BubbleKind, ManifestError, discover_conventional_bubbles, parse_package_manifest,
};

#[test]
fn minimal_manifest_has_typed_package_identity_and_sorted_dependencies() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         \n\
         [dependencies]\n\
         StudioData = \"2.1\"\n\
         AssetStore = \"1.4\"\n",
    )
    .expect("manifest");

    assert_eq!(manifest.name(), "Studio.Gameplay");
    assert_eq!(manifest.version(), "0.1.0");
    assert_eq!(manifest.edition(), "2026");
    assert_eq!(
        manifest
            .dependencies()
            .iter()
            .map(pop_projects::DependencyRequirement::alias)
            .collect::<Vec<_>>(),
        ["AssetStore", "StudioData"]
    );
}

#[test]
fn conventional_roots_create_non_overlapping_typed_bubbles() {
    let manifest = parse_package_manifest(
        "[package]\nname = \"Studio.Gameplay\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .expect("manifest");
    let files = [
        "src/lib.pop",
        "src/main.pop",
        "src/players.pop",
        "src/bin/migrate.pop",
        "src/bin/server/main.pop",
        "src/bin/server/routes.pop",
        "tests/saveRoundTrip.pop",
        "examples/basicServer.pop",
        "benchmarks/decoding.pop",
    ];
    let bubbles = discover_conventional_bubbles(&manifest, &files).expect("discovery");

    assert!(bubbles.iter().any(|bubble| {
        bubble.kind() == BubbleKind::Library
            && bubble.name() == "Studio.Gameplay"
            && bubble.modules() == ["src/lib.pop", "src/players.pop"]
    }));
    assert!(bubbles.iter().any(|bubble| {
        bubble.kind() == BubbleKind::Binary
            && bubble.name() == "Server"
            && bubble.modules() == ["src/bin/server/main.pop", "src/bin/server/routes.pop"]
    }));
    assert!(
        bubbles
            .iter()
            .filter(|bubble| bubble.depends_on_library())
            .all(|bubble| bubble.kind() != BubbleKind::Library)
    );
    let mut owned: Vec<_> = bubbles
        .iter()
        .flat_map(|bubble| bubble.modules().iter().map(String::as_str))
        .collect();
    owned.sort_unstable();
    owned.dedup();
    assert_eq!(owned.len(), files.len());
}

#[test]
fn noncanonical_manifest_and_target_names_are_rejected() {
    let manifest_error = parse_package_manifest(
        "[package]\nname = \"studio_gameplay\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .expect_err("package identity must be PascalCase components");
    assert_eq!(manifest_error, ManifestError::InvalidPackageName);

    let manifest = parse_package_manifest(
        "[package]\nname = \"Studio.Gameplay\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .expect("manifest");
    let error = discover_conventional_bubbles(&manifest, &["src/bin/asset_compiler.pop"])
        .expect_err("snake case target is not canonical");
    assert_eq!(error, ManifestError::InvalidTargetName);
}
