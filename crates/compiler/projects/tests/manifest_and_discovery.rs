use pop_projects::{
    BubbleKind, DependencySource, ManifestError, discover_conventional_bubbles,
    discover_workspace_members, parse_package_manifest, parse_workspace_manifest,
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

#[test]
fn structured_dependencies_preserve_exact_source_and_bubble_selection() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         \n\
         [dependencies]\n\
         StudioData = { path = \"../data\", version = \"2.1\", bubble = \"Studio.Data\" }\n\
         HttpCodec = { git = \"https://example.invalid/http-codec\", revision = \"8f31abcd\" }\n\
         SharedPolicy = { workspace = true }\n",
    )
    .expect("structured manifest");

    let dependencies = manifest.dependencies();
    assert_eq!(
        dependencies
            .iter()
            .map(pop_projects::DependencyRequirement::alias)
            .collect::<Vec<_>>(),
        ["HttpCodec", "SharedPolicy", "StudioData"]
    );
    assert_eq!(
        dependencies[0].source(),
        &DependencySource::ExactGit {
            repository: "https://example.invalid/http-codec".to_owned(),
            revision: "8f31abcd".to_owned(),
        }
    );
    assert!(dependencies[1].workspace_inherited());
    assert_eq!(
        dependencies[2].source(),
        &DependencySource::LocalPath("../data".to_owned())
    );
    assert_eq!(dependencies[2].version_requirement(), Some("2.1"));
    assert_eq!(dependencies[2].bubble(), Some("Studio.Data"));
}

#[test]
fn git_dependencies_fail_closed_without_an_exact_revision() {
    let error = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [dependencies]\n\
         HttpCodec = { git = \"https://example.invalid/http-codec\" }\n",
    )
    .expect_err("Git dependencies require an exact revision");

    assert_eq!(error, ManifestError::MissingGitRevision);
}

#[test]
fn workspace_members_expand_restricted_globs_deterministically() {
    let workspace = parse_workspace_manifest(
        "[workspace]\n\
         members = [\"packages/*\", \"tools/assetCompiler\"]\n\
         exclude = [\"packages/retired\"]\n\
         defaultMembers = [\"packages/gameplay\"]\n\
         resolver = \"1\"\n",
    )
    .expect("workspace manifest");

    assert_eq!(workspace.resolver(), "1");
    assert_eq!(workspace.default_members(), ["packages/gameplay"]);
    assert_eq!(
        discover_workspace_members(
            &workspace,
            &[
                "tools/assetCompiler",
                "packages/retired",
                "packages/data",
                "packages/gameplay",
                "unrelated/demo",
            ],
        )
        .expect("deterministic members"),
        ["packages/data", "packages/gameplay", "tools/assetCompiler"]
    );
}

#[test]
fn workspace_rejects_ambient_or_recursive_member_patterns() {
    for member in ["../outside", "/absolute", "packages/**"] {
        let manifest = format!("[workspace]\nmembers = [\"{member}\"]\nresolver = \"1\"\n");
        assert_eq!(
            parse_workspace_manifest(&manifest),
            Err(ManifestError::InvalidWorkspaceMember)
        );
    }
}

#[test]
fn one_root_manifest_can_be_both_package_and_workspace() {
    let text = "[package]\n\
                name = \"Studio.Root\"\n\
                version = \"0.1.0\"\n\
                edition = \"2026\"\n\
                [workspace]\n\
                members = [\"packages/*\"]\n\
                defaultMembers = [\"packages/application\"]\n\
                resolver = \"1\"\n";

    assert_eq!(
        parse_package_manifest(text).expect("root Package").name(),
        "Studio.Root"
    );
    assert_eq!(
        parse_workspace_manifest(text)
            .expect("root Workspace")
            .default_members(),
        ["packages/application"]
    );
}

#[test]
fn workspace_manifest_preserves_concrete_inherited_dependency_requirements() {
    let workspace = parse_workspace_manifest(
        "[workspace]\n\
         members = [\"packages/*\"]\n\
         resolver = \"1\"\n\
         [workspace.dependencies]\n\
         StudioData = { path = \"packages/data\", version = \"2.1.0\", bubble = \"Studio.Data\" }\n",
    )
    .expect("Workspace dependency requirement");

    let [dependency] = workspace.dependencies() else {
        panic!("one Workspace dependency");
    };
    assert_eq!(dependency.alias(), "StudioData");
    assert_eq!(dependency.version_requirement(), Some("2.1.0"));
    assert_eq!(dependency.bubble(), Some("Studio.Data"));
    assert_eq!(
        dependency.source(),
        &DependencySource::LocalPath("packages/data".to_owned())
    );
}

#[test]
fn development_and_platform_dependencies_remain_separate_scopes() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [developmentDependencies]\n\
         TestSupport = { path = \"../testSupport\", version = \"1.0.0\" }\n\
         [platform.\"x86_64-linux\".dependencies]\n\
         NativeTls = \"1.4\"\n",
    )
    .expect("scoped dependencies");

    assert!(manifest.dependencies().is_empty());
    assert_eq!(
        manifest.development_dependencies()[0].alias(),
        "TestSupport"
    );
    assert_eq!(manifest.platform_dependencies().len(), 1);
    assert_eq!(
        manifest.platform_dependencies()[0].platform_target(),
        "x86_64-linux"
    );
    assert_eq!(
        manifest.platform_dependencies()[0].dependencies()[0].alias(),
        "NativeTls"
    );
}

#[test]
fn dependency_selection_merges_only_the_exact_platform_and_requested_scope() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [dependencies]\n\
         StudioData = { path = \"../data\", version = \"1.0.0\" }\n\
         [developmentDependencies]\n\
         TestSupport = { path = \"../testSupport\", version = \"1.0.0\" }\n\
         [platform.\"x86_64-unknown-linux-gnu\".dependencies]\n\
         NativeTls = { path = \"../nativeTls\", version = \"1.0.0\" }\n\
         [platform.\"aarch64-apple-darwin\".dependencies]\n\
         AppleSecurity = { path = \"../appleSecurity\", version = \"1.0.0\" }\n",
    )
    .expect("scoped manifest");

    assert_eq!(
        manifest
            .selected_dependencies("x86_64-unknown-linux-gnu", false)
            .expect("production dependency selection")
            .iter()
            .map(|dependency| dependency.alias())
            .collect::<Vec<_>>(),
        ["NativeTls", "StudioData"]
    );
    assert_eq!(
        manifest
            .selected_dependencies("x86_64-unknown-linux-gnu", true)
            .expect("development dependency selection")
            .iter()
            .map(|dependency| dependency.alias())
            .collect::<Vec<_>>(),
        ["NativeTls", "StudioData", "TestSupport"]
    );
}

#[test]
fn dependency_selection_rejects_aliases_duplicated_across_active_scopes() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [dependencies]\n\
         StudioData = { path = \"../data\", version = \"1.0.0\" }\n\
         [platform.\"x86_64-unknown-linux-gnu\".dependencies]\n\
         StudioData = { path = \"../otherData\", version = \"1.0.0\" }\n",
    )
    .expect("scope-local parsing remains deterministic");

    assert_eq!(
        manifest.selected_dependencies("x86_64-unknown-linux-gnu", false),
        Err(ManifestError::DuplicateDependencyAlias)
    );
}

#[test]
fn feature_selection_enables_only_declared_optional_dependencies() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [features]\n\
         telemetry = [\"dependency:Telemetry\"]\n\
         [dependencies]\n\
         StudioData = { path = \"../data\", version = \"1.0.0\" }\n\
         Telemetry = { path = \"../telemetry\", version = \"1.0.0\", optional = true }\n",
    )
    .expect("feature manifest");

    assert_eq!(manifest.features()[0].name(), "telemetry");
    assert!(manifest.dependencies()[1].optional());
    assert_eq!(
        manifest
            .selected_dependencies_with_features(
                "x86_64-unknown-linux-gnu",
                false,
                std::iter::empty::<&str>(),
            )
            .expect("unfeatured selection")
            .iter()
            .map(|dependency| dependency.alias())
            .collect::<Vec<_>>(),
        ["StudioData"]
    );
    assert_eq!(
        manifest
            .selected_dependencies_with_features("x86_64-unknown-linux-gnu", false, ["telemetry"],)
            .expect("featured selection")
            .iter()
            .map(|dependency| dependency.alias())
            .collect::<Vec<_>>(),
        ["StudioData", "Telemetry"]
    );
}

#[test]
fn feature_manifest_rejects_unknown_duplicate_and_nonoptional_activations() {
    let prefix = "[package]\n\
                  name = \"Studio.Gameplay\"\n\
                  version = \"0.1.0\"\n\
                  edition = \"2026\"\n";
    for (body, expected) in [
        (
            "[features]\ntelemetry = [\"dependency:Missing\"]\n",
            ManifestError::UnknownFeatureDependency,
        ),
        (
            "[features]\ntelemetry = [\"dependency:Telemetry\", \"dependency:Telemetry\"]\n\
             [dependencies]\nTelemetry = { version = \"1.0.0\", optional = true }\n",
            ManifestError::DuplicateFeatureMember,
        ),
        (
            "[features]\ntelemetry = [\"dependency:Telemetry\"]\n\
             [dependencies]\nTelemetry = \"1.0.0\"\n",
            ManifestError::FeatureDependencyNotOptional,
        ),
        (
            "[features]\nTelemetry = []\n",
            ManifestError::InvalidFeature,
        ),
        (
            "[features]\ntelemetry = []\n\
             [dependencies]\nTelemetry = { version = \"1.0.0\", optional = true }\n",
            ManifestError::UnreferencedOptionalDependency,
        ),
    ] {
        assert_eq!(
            parse_package_manifest(&format!("{prefix}{body}")),
            Err(expected),
            "{body}"
        );
    }
}

#[test]
fn feature_selection_rejects_unknown_and_duplicate_requested_names() {
    let manifest = parse_package_manifest(
        "[package]\n\
         name = \"Studio.Gameplay\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [features]\n\
         telemetry = []\n",
    )
    .expect("feature manifest");

    assert_eq!(
        manifest.selected_dependencies_with_features(
            "x86_64-unknown-linux-gnu",
            false,
            ["missing"],
        ),
        Err(ManifestError::UnknownFeature)
    );
    assert_eq!(
        manifest.selected_dependencies_with_features(
            "x86_64-unknown-linux-gnu",
            false,
            ["telemetry", "telemetry"],
        ),
        Err(ManifestError::DuplicateSelectedFeature)
    );
}
