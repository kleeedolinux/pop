//! Conformance tests for ADR 0018 and architecture section 11.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use pop_projects::{BubbleKind, discover_conventional_bubbles, parse_package_manifest};

const MEMBERS: &[&str] = &[
    "crates/compiler/backend-api",
    "crates/compiler/backends/c",
    "crates/compiler/backends/llvm",
    "crates/compiler/backends/mir-interp",
    "crates/compiler/backends/vm",
    "crates/compiler/compile-time",
    "crates/compiler/diagnostics",
    "crates/compiler/documentation",
    "crates/compiler/driver",
    "crates/compiler/foundation",
    "crates/compiler/hir",
    "crates/compiler/mir",
    "crates/compiler/projects",
    "crates/compiler/query",
    "crates/compiler/resolve",
    "crates/compiler/source",
    "crates/compiler/syntax",
    "crates/compiler/target",
    "crates/compiler/types",
    "crates/extensions/ai",
    "crates/extensions/cli",
    "crates/extensions/data",
    "crates/extensions/lsp",
    "crates/extensions/rpc",
    "crates/extensions/syntax",
    "crates/libraries/internal",
    "crates/libraries/standard",
    "crates/runtime/interface",
    "crates/runtime/native",
    "crates/tools/architecture-tests",
    "crates/tools/documentation-generator",
    "crates/tools/formatter",
    "crates/tools/language-server",
    "crates/tools/test-runner",
];

const PUBLIC_LIBRARY_ROOTS: &[&str] = &[
    "Ai",
    "Archive",
    "Atomic",
    "Audio",
    "Benchmark",
    "Bytes",
    "Channel",
    "Cli",
    "Codec",
    "Command",
    "Compress",
    "Crypto",
    "Csv",
    "Data",
    "Device",
    "Diagnostic",
    "Directory",
    "Documentation",
    "Email",
    "Environment",
    "Ffi",
    "File",
    "Geometry",
    "Glob",
    "Graphics",
    "Guid",
    "Http",
    "Identity",
    "Image",
    "Io",
    "Json",
    "Locale",
    "Lsp",
    "Math",
    "Media",
    "Memory",
    "Message",
    "Metadata",
    "Mime",
    "Net",
    "Package",
    "Path",
    "Platform",
    "Process",
    "Random",
    "Regex",
    "Resource",
    "Rpc",
    "Schedule",
    "Science",
    "Sequence",
    "Settings",
    "Signal",
    "Socket",
    "Source",
    "Sql",
    "Statistics",
    "Store",
    "Syntax",
    "Task",
    "Telemetry",
    "Tensor",
    "Terminal",
    "Test",
    "Text",
    "Time",
    "Toml",
    "Ui",
    "Unicode",
    "Units",
    "Uri",
    "Version",
    "Video",
    "WebSocket",
    "Xml",
    "Yaml",
];

const PUBLIC_LIBRARY_CATALOGS: &[&str] = &[
    "22.1-core-and-portable-library-catalog.md",
    "22.2-system-network-security-catalog.md",
    "22.3-data-observability-tooling-catalog.md",
    "22.4-application-media-science-catalog.md",
];

struct ExtensionExpectation {
    directory: &'static str,
    package: &'static str,
    cargo_package: &'static str,
    sources: &'static [(&'static str, &'static str)],
    dependencies: &'static [&'static str],
}

const OFFICIAL_EXTENSIONS: &[ExtensionExpectation] = &[
    ExtensionExpectation {
        directory: "data",
        package: "Pop.Data",
        cargo_package: "pop-extension-data",
        sources: &[
            ("src/lib.pop", "namespace Pop.Data"),
            ("src/sql.pop", "namespace Pop.Sql"),
            ("src/store.pop", "namespace Pop.Store"),
        ],
        dependencies: &[],
    },
    ExtensionExpectation {
        directory: "ai",
        package: "Pop.Ai",
        cargo_package: "pop-extension-ai",
        sources: &[("src/lib.pop", "namespace Pop.Ai")],
        dependencies: &["PopData"],
    },
    ExtensionExpectation {
        directory: "cli",
        package: "Pop.Cli",
        cargo_package: "pop-extension-cli",
        sources: &[
            ("src/lib.pop", "namespace Pop.Cli"),
            ("src/command.pop", "namespace Pop.Command"),
            ("src/settings.pop", "namespace Pop.Settings"),
        ],
        dependencies: &[],
    },
    ExtensionExpectation {
        directory: "rpc",
        package: "Pop.Rpc",
        cargo_package: "pop-extension-rpc",
        sources: &[("src/lib.pop", "namespace Pop.Rpc")],
        dependencies: &[],
    },
    ExtensionExpectation {
        directory: "syntax",
        package: "Pop.Syntax",
        cargo_package: "pop-extension-syntax",
        sources: &[
            ("src/lib.pop", "namespace Pop.Syntax"),
            ("src/source.pop", "namespace Pop.Source"),
        ],
        dependencies: &[],
    },
    ExtensionExpectation {
        directory: "lsp",
        package: "Pop.Lsp",
        cargo_package: "pop-extension-lsp",
        sources: &[("src/lib.pop", "namespace Pop.Lsp")],
        dependencies: &["PopRpc", "PopSyntax"],
    },
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("architecture-tests must remain under crates/tools")
        .to_path_buf()
}

fn quoted_values_in_array(manifest: &str, key: &str) -> BTreeSet<String> {
    let key_start = manifest
        .find(key)
        .unwrap_or_else(|| panic!("missing `{key}` in root manifest"));
    let array_start = manifest[key_start..]
        .find('[')
        .map(|offset| key_start + offset)
        .expect("workspace members must be an array");
    let array_end = manifest[array_start..]
        .find(']')
        .map(|offset| array_start + offset)
        .expect("workspace members array must be closed");

    manifest[array_start + 1..array_end]
        .split(',')
        .filter_map(|item| {
            let item = item.trim();
            item.strip_prefix('"')
                .and_then(|item| item.strip_suffix('"'))
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn workspace_has_the_accepted_member_inventory() {
    let root = repository_root();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read root Cargo.toml");
    let actual = quoted_values_in_array(&manifest, "members");
    let expected = MEMBERS.iter().map(|member| (*member).to_owned()).collect();

    assert_eq!(actual, expected, "ADR 0018 crate inventory drifted");
}

#[test]
fn every_member_inherits_workspace_metadata_and_has_a_target() {
    let root = repository_root();

    for member in MEMBERS {
        let directory = root.join(member);
        let manifest_path = directory.join("Cargo.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", manifest_path.display()));

        assert!(
            manifest.contains("name = \"pop-"),
            "{} package name must use the pop- prefix",
            manifest_path.display()
        );

        let version_contract = if member.starts_with("crates/extensions/") {
            "version = \"0.1.0\""
        } else {
            "version.workspace = true"
        };
        assert!(
            manifest.contains(version_contract),
            "{} must contain `{version_contract}`",
            manifest_path.display()
        );

        for inherited in [
            "edition.workspace = true",
            "rust-version.workspace = true",
            "license.workspace = true",
            "[lints]",
            "workspace = true",
        ] {
            assert!(
                manifest.contains(inherited),
                "{} must contain `{inherited}`",
                manifest_path.display()
            );
        }

        let has_target =
            directory.join("src/lib.rs").is_file() || directory.join("src/main.rs").is_file();
        assert!(has_target, "{} has no Rust target", directory.display());
    }
}

#[test]
fn dependencies_are_centralized_and_external_dependencies_are_approved() {
    let root = repository_root();
    let root_manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read root Cargo.toml");
    let dependency_table = root_manifest
        .split_once("[workspace.dependencies]")
        .expect("workspace dependency table")
        .1
        .split_once("[workspace.lints.rust]")
        .expect("workspace lint table after dependencies")
        .0;

    for line in dependency_table
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let local =
            line.starts_with("pop-") && line.contains(" = { path = \"") && line.ends_with("\" }");
        let approved_inkwell = line
            == "inkwell = { version = \"0.9.0\", default-features = false, features = [\"llvm22-1-prefer-dynamic\", \"target-x86\"] }";
        assert!(
            local || approved_inkwell,
            "unapproved workspace dependency: {line}"
        );
    }

    for member in MEMBERS {
        let manifest_path = root.join(member).join("Cargo.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", manifest_path.display()));
        for table in ["[dependencies]", "[dev-dependencies]"] {
            let Some((_, dependencies)) = manifest.split_once(table) else {
                continue;
            };
            let dependencies = dependencies
                .find("\n[")
                .map_or(dependencies, |end| &dependencies[..end]);
            for line in dependencies.lines().filter(|line| !line.trim().is_empty()) {
                let inherited_local =
                    line.starts_with("pop-") && line.ends_with(".workspace = true");
                let inherited_inkwell = *member == "crates/compiler/backends/llvm"
                    && line == "inkwell.workspace = true";
                assert!(
                    inherited_local || inherited_inkwell,
                    "{} {table} entry is not inherited from the workspace: {line}",
                    manifest_path.display(),
                );
            }
        }
    }
}

#[test]
fn inkwell_is_confined_to_the_llvm_backend() {
    let root = repository_root();
    for member in MEMBERS {
        let manifest_path = root.join(member).join("Cargo.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", manifest_path.display()));
        assert_eq!(
            manifest.contains("inkwell.workspace = true"),
            *member == "crates/compiler/backends/llvm",
            "Inkwell must remain private to pop-backend-llvm"
        );
    }
}

#[test]
fn portable_crates_do_not_name_backend_packages() {
    let root = repository_root();
    let portable = [
        "crates/compiler/foundation",
        "crates/compiler/source",
        "crates/compiler/syntax",
        "crates/compiler/projects",
        "crates/compiler/query",
        "crates/compiler/resolve",
        "crates/compiler/types",
        "crates/compiler/compile-time",
        "crates/compiler/hir",
        "crates/compiler/mir",
        "crates/compiler/target",
        "crates/runtime/interface",
    ];

    for member in portable {
        let manifest_path = root.join(member).join("Cargo.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", manifest_path.display()));
        for forbidden in [
            "pop-backend-llvm",
            "pop-backend-c",
            "pop-backend-mir-interp",
            "pop-backend-vm",
        ] {
            assert!(
                !manifest.contains(forbidden),
                "{} must not depend on {forbidden}",
                manifest_path.display()
            );
        }
    }
}

#[test]
fn reserved_library_layers_have_the_required_dependency_direction() {
    let root = repository_root();
    let internal = fs::read_to_string(root.join("crates/libraries/internal/Cargo.toml"))
        .expect("read Pop.Internal implementation manifest");
    let standard = fs::read_to_string(root.join("crates/libraries/standard/Cargo.toml"))
        .expect("read Pop.Standard implementation manifest");
    assert!(standard.contains("pop-internal.workspace = true"));
    assert!(!internal.contains("pop-standard.workspace = true"));
}

#[test]
fn official_extensions_have_independent_manifests_namespaces_and_builds() {
    let root = repository_root();

    for extension in OFFICIAL_EXTENSIONS {
        let directory = root.join("crates/extensions").join(extension.directory);
        let manifest_path = directory.join("bubble.toml");
        let manifest_text = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", manifest_path.display()));
        let manifest = parse_package_manifest(&manifest_text)
            .unwrap_or_else(|error| panic!("cannot parse {}: {error}", manifest_path.display()));
        assert_eq!(manifest.name(), extension.package);
        assert_eq!(manifest.version(), "0.1.0");
        assert_eq!(manifest.edition(), "2026");
        assert_eq!(
            manifest
                .dependencies()
                .iter()
                .map(pop_projects::DependencyRequirement::alias)
                .collect::<Vec<_>>(),
            extension.dependencies
        );

        let source_paths: Vec<_> = extension.sources.iter().map(|(path, _)| *path).collect();
        let bubbles = discover_conventional_bubbles(&manifest, &source_paths)
            .unwrap_or_else(|error| panic!("cannot discover {}: {error}", manifest_path.display()));
        assert_eq!(bubbles.len(), 1);
        assert_eq!(bubbles[0].kind(), BubbleKind::Library);
        assert_eq!(bubbles[0].name(), extension.package);

        for (source_path, namespace) in extension.sources {
            let path = directory.join(source_path);
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            assert!(
                source.lines().any(|line| line.trim() == *namespace),
                "{} must own `{namespace}`",
                path.display()
            );
        }

        assert_extension_cargo_manifest(&directory, extension);
    }
}

fn assert_extension_cargo_manifest(directory: &Path, extension: &ExtensionExpectation) {
    let path = directory.join("Cargo.toml");
    let manifest = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    assert!(manifest.contains(&format!("name = \"{}\"", extension.cargo_package)));
    assert!(manifest.contains("version = \"0.1.0\""));
    assert!(
        !manifest
            .lines()
            .any(|line| line.trim() == "version.workspace = true")
    );
    for private in [
        "pop-query.workspace = true",
        "pop-source.workspace = true",
        "pop-syntax.workspace = true",
        "pop-types.workspace = true",
    ] {
        assert!(
            !manifest.contains(private),
            "{} must not depend on private `{private}`",
            path.display()
        );
    }

    let actual_dependencies: BTreeSet<_> = manifest
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_suffix(".workspace = true")
                .filter(|name| name.starts_with("pop-extension-"))
        })
        .collect();
    let expected_dependencies: BTreeSet<_> = extension
        .dependencies
        .iter()
        .map(|alias| match *alias {
            "PopData" => "pop-extension-data",
            "PopRpc" => "pop-extension-rpc",
            "PopSyntax" => "pop-extension-syntax",
            _ => panic!("unknown official extension dependency alias `{alias}`"),
        })
        .collect();
    assert_eq!(
        actual_dependencies, expected_dependencies,
        "{} Cargo and Pop dependency graphs disagree",
        extension.package
    );
}

#[test]
fn official_extensions_are_absent_from_the_standard_bootstrap() {
    let root = repository_root();
    let mut bootstrap = String::new();
    for file in [
        "functions.tsv",
        "prelude-types.tsv",
        "compiler-attributes.tsv",
    ] {
        bootstrap.push_str(
            &fs::read_to_string(root.join("libraries/standard/bootstrap").join(file))
                .unwrap_or_else(|error| panic!("cannot read standard bootstrap {file}: {error}")),
        );
    }
    for extension in OFFICIAL_EXTENSIONS {
        assert!(
            !bootstrap.contains(extension.package),
            "{} must not enter Pop.Standard bootstrap metadata",
            extension.package
        );
    }
}

#[test]
fn official_language_server_uses_the_pop_lsp_protocol_boundary() {
    let root = repository_root();
    let manifest = fs::read_to_string(root.join("crates/tools/language-server/Cargo.toml"))
        .expect("read language-server manifest");
    let source = fs::read_to_string(root.join("crates/tools/language-server/src/lib.rs"))
        .expect("read language-server source");
    assert!(manifest.contains("pop-extension-lsp.workspace = true"));
    assert!(source.contains("pop_extension_lsp::PACKAGE"));
}

#[test]
fn driver_declares_the_pop_binary() {
    let root = repository_root();
    let manifest_path = root.join("crates/compiler/driver/Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("read pop-driver manifest");

    assert!(manifest.contains("name = \"pop\""));
    assert!(manifest.contains("path = \"src/main.rs\""));
}

#[test]
fn active_public_library_contract_is_native_and_tiered() {
    let root = repository_root();
    let catalog_path = root.join("architecture/22-public-standard-library-architecture.md");
    let catalog = fs::read_to_string(&catalog_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", catalog_path.display()));

    for required in [
        "ADRs 0030, 0031, and 0032",
        "## Usability contract",
        "## Cost contract",
        "## Distribution tiers",
        "## Catalog map",
        "## Status vocabulary",
        "## Dependency direction",
    ] {
        assert!(
            catalog.contains(required),
            "public-library catalog must contain `{required}`"
        );
    }

    for path in [
        "AGENTS.md",
        "architecture/README.md",
        "architecture/01-vision-and-principles.md",
        "architecture/08.1-closed-design-questions.md",
    ] {
        let document_path = root.join(path);
        let document = fs::read_to_string(&document_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", document_path.display()));
        assert!(
            !document.contains("BCL-inspired"),
            "{path} must not retain an active BCL-inspired public-library contract"
        );
    }

    let base_libraries_path = root.join("architecture/16-base-libraries.md");
    let base_libraries = fs::read_to_string(&base_libraries_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", base_libraries_path.display()));
    assert!(base_libraries.contains("## Historical influence boundary"));
    assert!(base_libraries.contains("./22-public-standard-library-architecture.md"));
}

#[test]
fn public_library_catalog_has_one_complete_root_inventory() {
    let root = repository_root();
    let index_path = root.join("architecture/22-public-standard-library-architecture.md");
    let index = fs::read_to_string(&index_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", index_path.display()));
    let (actual, inventory_tiers, inventory) = parse_root_inventory(&index);
    let expected = PUBLIC_LIBRARY_ROOTS
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(actual, expected, "planned public root inventory drifted");

    let documented = documented_catalog_roots(&root, &inventory_tiers);
    assert_eq!(
        documented, expected,
        "every planned root must have exactly one domain catalog row"
    );
    assert_supporting_catalog_documents(&root);
    assert_example_contract(&root);

    for stale in [
        "Data.Json",
        "Text.Pattern",
        "`Term`",
        "`Observe`",
        "`System`",
    ] {
        assert!(
            !inventory.contains(stale),
            "stale first-draft root `{stale}`"
        );
    }
}

fn parse_root_inventory(index: &str) -> (BTreeSet<String>, BTreeMap<String, String>, &str) {
    let start = index
        .find("<!-- namespace-roots:start -->")
        .expect("namespace root inventory start marker");
    let end = index
        .find("<!-- namespace-roots:end -->")
        .expect("namespace root inventory end marker");
    let inventory = &index[start..end];

    let mut actual = BTreeSet::new();
    let mut inventory_tiers = BTreeMap::new();
    for line in inventory.lines().filter(|line| line.starts_with("| `")) {
        let columns: Vec<_> = line.split('|').map(str::trim).collect();
        let name = line
            .split('`')
            .nth(1)
            .expect("catalog root rows must start with a quoted name");
        assert!(
            actual.insert(name.to_owned()),
            "duplicate catalog root: {name}"
        );
        assert!(
            inventory_tiers
                .insert(name.to_owned(), columns[2].to_owned())
                .is_none(),
            "duplicate tier assignment for `{name}`"
        );
    }
    (actual, inventory_tiers, inventory)
}

fn documented_catalog_roots(
    root: &Path,
    inventory_tiers: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut documented = BTreeSet::new();
    for document in PUBLIC_LIBRARY_CATALOGS {
        let path = root.join("architecture").join(document);
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for line in contents.lines().filter(|line| line.starts_with("| `")) {
            let columns: Vec<_> = line.split('|').map(str::trim).collect();
            let name = line
                .split('`')
                .nth(1)
                .expect("catalog rows must start with a quoted root name");
            assert!(
                documented.insert(name.to_owned()),
                "root `{name}` is documented in more than one domain catalog"
            );
            assert!(
                line.contains("planned")
                    || line.contains("prototype")
                    || line.contains("implemented"),
                "catalog row for `{name}` lacks an explicit status"
            );
            let expected_tier = inventory_tiers
                .get(name)
                .unwrap_or_else(|| panic!("domain catalog contains unknown root `{name}`"));
            assert!(
                columns[2].starts_with(expected_tier),
                "tier for `{name}` is `{}`, expected `{expected_tier}`",
                columns[2]
            );
        }
    }
    documented
}

fn assert_supporting_catalog_documents(root: &Path) {
    for document in [
        "22.5-standard-library-api-examples.md",
        "22.6-standard-library-implementation-plan.md",
    ] {
        let path = root.join("architecture").join(document);
        assert!(
            path.is_file(),
            "missing public-library catalog document: {document}"
        );
    }
}

fn assert_example_contract(root: &Path) {
    let examples_path = root.join("architecture/22.5-standard-library-api-examples.md");
    let examples = fs::read_to_string(&examples_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", examples_path.display()));
    assert!(examples.contains("There is no `?`, `await`, `defer`,"));
    assert!(examples.contains("Proposed syntax only."));
    for stale in ["Data.Json", "Text.Pattern", "Async.run", "Io.open"] {
        assert!(
            !examples.contains(stale),
            "examples retain stale API `{stale}`"
        );
    }
}
