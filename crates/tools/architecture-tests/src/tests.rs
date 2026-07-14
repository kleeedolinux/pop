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
    "crates/libraries/bridge",
    "crates/libraries/internal",
    "crates/libraries/macros",
    "crates/libraries/standard",
    "crates/runtime/collector",
    "crates/runtime/interface",
    "crates/runtime/native-abi",
    "crates/runtime/native",
    "crates/tools/architecture-tests",
    "crates/tools/documentation-generator",
    "crates/tools/formatter",
    "crates/tools/language-server",
    "crates/tools/test-runner",
];

const PUBLIC_LIBRARY_ROOTS: &[&str] = &[
    "Actor",
    "Ai",
    "Archive",
    "Atomic",
    "Audio",
    "Benchmark",
    "Bytes",
    "Channel",
    "Cli",
    "Cluster",
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

fn collect_text_contract_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .map(|entry| entry.expect("read repository entry").path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect_text_contract_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| matches!(extension.to_str(), Some("md" | "pop" | "rs")))
        {
            files.push(path);
        }
    }
}

fn inline_documentation_element(line: &str) -> Option<&str> {
    let mut remainder = line;
    while let Some(start) = remainder.find("--- <") {
        let candidate = &remainder[start + 5..];
        let name_end = candidate
            .find(|character: char| {
                character.is_ascii_whitespace() || matches!(character, '>' | '/')
            })
            .unwrap_or(candidate.len());
        let name = &candidate[..name_end];
        let valid_name = name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
            && name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':' | '.')
            });
        if valid_name {
            let opening_end = candidate.find('>')?;
            if !candidate[..opening_end].ends_with('/')
                && let Some(closing_start) = candidate[opening_end + 1..].find("</")
            {
                let body = &candidate[opening_end + 1..opening_end + 1 + closing_start];
                if !body.contains("\\n") {
                    let closing_end = candidate[opening_end + 1 + closing_start..]
                        .find('>')
                        .map_or(candidate.len(), |end| {
                            opening_end + 1 + closing_start + end + 1
                        });
                    let end = start + 5 + closing_end;
                    return Some(&remainder[start..end]);
                }
            }
        }
        remainder = &candidate[1.min(candidate.len())..];
    }
    None
}

#[test]
fn xml_documentation_body_is_never_inline() {
    let root = repository_root();
    let mut files = Vec::new();
    collect_text_contract_files(&root, &mut files);

    let mut violations = Vec::new();
    for path in files {
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for (index, line) in contents.lines().enumerate() {
            if let Some(element) = inline_documentation_element(line) {
                violations.push(format!(
                    "{}:{}: {element}",
                    path.strip_prefix(&root).unwrap_or(&path).display(),
                    index + 1
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "ADR 0057 forbids inline XML documentation bodies:\n{}",
        violations.join("\n")
    );
}

#[test]
fn accepted_adrs_have_unique_numeric_identities() {
    let decisions = repository_root().join("architecture/decisions");
    let mut paths = fs::read_dir(&decisions)
        .expect("read architecture decisions")
        .map(|entry| entry.expect("read decision entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut identities = BTreeMap::new();
    for path in paths {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("decision file name is UTF-8");
        let Some((identity, _)) = file_name.split_once('-') else {
            continue;
        };
        if identity.len() != 4 || !identity.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if let Some(previous) = identities.insert(identity.to_owned(), file_name.to_owned()) {
            panic!("duplicate ADR identity {identity}: {previous} and {file_name}");
        }
    }
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
            == "inkwell = { version = \"0.9.0\", default-features = false, features = [\"llvm22-1-prefer-dynamic\", \"target-x86\", \"target-bpf\"] }";
        let approved_artifact_dependency = matches!(
            line,
            "serde = { version = \"1.0.228\", features = [\"derive\"] }"
                | "serde_json = \"1.0.150\""
                | "sha2 = \"0.11.0\""
        );
        assert!(
            local || approved_inkwell || approved_artifact_dependency,
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
                let serde_projection = matches!(
                    *member,
                    "crates/compiler/foundation"
                        | "crates/compiler/resolve"
                        | "crates/compiler/types"
                        | "crates/compiler/hir"
                ) && line == "serde.workspace = true";
                let project_artifact_dependency = *member == "crates/compiler/projects"
                    && matches!(
                        line,
                        "serde.workspace = true"
                            | "serde_json.workspace = true"
                            | "sha2.workspace = true"
                    );
                let driver_artifact_dependency = *member == "crates/compiler/driver"
                    && matches!(
                        line,
                        "serde.workspace = true"
                            | "serde_json.workspace = true"
                            | "sha2.workspace = true"
                    );
                assert!(
                    inherited_local
                        || inherited_inkwell
                        || serde_projection
                        || project_artifact_dependency
                        || driver_artifact_dependency,
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
        "crates/runtime/collector",
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
fn runtime_crates_enforce_adr_0038_ownership() {
    let root = repository_root();
    let runtime = root.join("crates/runtime");

    assert_plri_boundary(&runtime);
    assert_collector_boundary(&runtime);
    assert_native_abi_boundary(&runtime);
    assert_native_facade_boundary(&runtime);
    assert_runtime_benchmark_contract(&runtime);
    assert_runtime_consumer_dependencies(&root);
    assert_runtime_documentation(&runtime);
}

fn assert_plri_boundary(runtime: &Path) {
    let interface_manifest = read_required(runtime.join("interface/Cargo.toml"));
    let interface_source = read_rust_sources(&runtime.join("interface/src"));
    assert!(
        !interface_manifest.contains("[dependencies]"),
        "PLRI must remain an implementation-independent leaf contract"
    );
    for forbidden in [
        "pop_rt_",
        "extern \"C\"",
        "OnceLock",
        "BootstrapRuntime",
        "std::ffi",
    ] {
        assert!(
            !interface_source.contains(forbidden),
            "PLRI source must not contain native/implementation detail `{forbidden}`"
        );
    }
    for required in [
        "root_values_mut",
        "roots: &mut RootPublication",
        "RelocationConformance",
        "relocation_conformance_stage2",
    ] {
        assert!(
            interface_source.contains(required),
            "PLRI must preserve relocation contract `{required}`"
        );
    }
}

fn assert_collector_boundary(runtime: &Path) {
    let collector_manifest = read_required(runtime.join("collector/Cargo.toml"));
    let collector_source = read_rust_sources(&runtime.join("collector/src"));
    let relocation_source = read_rust_sources(&runtime.join("collector/src/relocation"));
    assert!(collector_manifest.contains("pop-runtime-interface.workspace = true"));
    assert!(!collector_manifest.contains("pop-runtime-native-abi.workspace = true"));
    assert!(!collector_manifest.contains("pop-runtime-native.workspace = true"));
    assert!(collector_source.contains("impl RuntimeAdapter for BootstrapRuntime"));
    assert!(relocation_source.contains("impl RuntimeAdapter for RelocationRuntime"));
    for forbidden in ["pop_rt_", "extern \"C\"", "OnceLock", "std::ffi"] {
        assert!(
            !collector_source.contains(forbidden),
            "portable collector must not contain native detail `{forbidden}`"
        );
    }
    let collector_root = read_required(runtime.join("collector/src/lib.rs"));
    for declaration in [
        "mod access;",
        "mod adapter;",
        "mod heap;",
        "mod relocation;",
        "mod trace;",
    ] {
        assert!(
            collector_root
                .lines()
                .any(|line| line.trim() == declaration),
            "collector root must explicitly inventory `{declaration}`"
        );
    }
    assert!(!collector_root.contains("mod bootstrap;"));
    assert!(!runtime.join("collector/src/bootstrap.rs").exists());
    let relocation_root = read_required(runtime.join("collector/src/relocation/mod.rs"));
    for declaration in ["mod adapter;", "mod collect;", "mod heap;"] {
        assert!(
            relocation_root
                .lines()
                .any(|line| line.trim() == declaration),
            "relocation collector root must explicitly inventory `{declaration}`"
        );
    }
    assert_runtime_modules_are_focused(&runtime.join("collector/src"), 320);
    assert_runtime_modules_are_focused(&runtime.join("collector/src/relocation"), 320);
}

fn assert_native_abi_boundary(runtime: &Path) {
    let native_abi_manifest = read_required(runtime.join("native-abi/Cargo.toml"));
    let native_abi_source = read_rust_sources(&runtime.join("native-abi/src"));
    assert!(native_abi_manifest.contains("pop-runtime-interface.workspace = true"));
    assert!(!native_abi_manifest.contains("pop-runtime-collector.workspace = true"));
    assert!(native_abi_source.contains("pop_rt_"));
    assert!(native_abi_source.contains("RuntimeOperation"));
    assert!(native_abi_source.contains("Option<&'static str>"));
    assert!(!native_abi_source.contains("_ => Some("));
}

fn assert_native_facade_boundary(runtime: &Path) {
    let native_manifest = read_required(runtime.join("native/Cargo.toml"));
    for dependency in [
        "pop-runtime-interface.workspace = true",
        "pop-runtime-collector.workspace = true",
        "pop-runtime-native-abi.workspace = true",
    ] {
        assert!(
            native_manifest.contains(dependency),
            "native facade must depend on `{dependency}`"
        );
    }
    let native_root = read_required(runtime.join("native/src/lib.rs"));
    let native_state = read_required(runtime.join("native/src/state.rs"));
    assert!(native_state.contains("StableGenerationalRuntime"));
    assert!(!native_state.contains("BootstrapRuntime"));
    for declaration in [
        "mod allocation;",
        "mod failure;",
        "mod identity;",
        "mod roots;",
        "mod state;",
        "mod storage;",
        "mod text;",
    ] {
        assert!(
            native_root.lines().any(|line| line.trim() == declaration),
            "native facade root must explicitly inventory `{declaration}`"
        );
    }
    assert!(!native_root.contains("mod abi;"));
    assert!(!runtime.join("native/src/abi.rs").exists());
    assert_runtime_modules_are_focused(&runtime.join("native/src"), 320);
}

fn assert_runtime_benchmark_contract(runtime: &Path) {
    let collector_manifest = read_required(runtime.join("collector/Cargo.toml"));
    for path in [
        "collector/benches/bootstrap.rs",
        "collector/benches/relocation.rs",
        "collector/benches/relocation_workload.rs",
        "collector/benches/workload.rs",
        "collector/benches/workload/array.rs",
        "collector/benches/workload/model.rs",
        "collector/benches/workload/pin.rs",
        "collector/benches/workload/pressure.rs",
        "collector/benches/workload/rooted.rs",
        "collector/benches/workload/tiny.rs",
        "collector/tests/benchmark_contract.rs",
        "collector/tests/metrics.rs",
        "collector/tests/relocating_nursery.rs",
        "collector/tests/relocation_benchmark_contract.rs",
    ] {
        assert!(
            runtime.join(path).is_file(),
            "ADR 0038 runtime benchmark file `{path}` is missing"
        );
    }
    for required in [
        "[[bench]]",
        "name = \"bootstrap\"",
        "name = \"relocation\"",
        "harness = false",
    ] {
        assert!(
            collector_manifest.contains(required),
            "collector manifest must contain benchmark contract `{required}`"
        );
    }
    let workload_root = read_required(runtime.join("collector/benches/workload.rs"));
    for declaration in [
        "mod array;",
        "mod model;",
        "mod pin;",
        "mod pressure;",
        "mod rooted;",
        "mod tiny;",
    ] {
        assert!(
            workload_root.contains(declaration),
            "benchmark workload root must inventory `{declaration}`"
        );
    }
    assert_runtime_modules_are_focused(&runtime.join("collector/benches/workload"), 250);
    let benchmark = read_required(runtime.join("collector/benches/bootstrap.rs"));
    for field in [
        "schema",
        "profile",
        "target_architecture",
        "target_operating_system",
        "build_profile",
        "collector_stage",
        "workload",
        "samples",
        "operations",
        "allocations",
        "reference_stores",
        "root_transitions",
        "pin_transitions",
        "elapsed_nanoseconds",
        "nanoseconds_per_operation",
        "available_parallelism",
        "logical_peak_objects",
        "logical_peak_slots",
        "collections",
        "reclaimed_objects",
        "scanned_objects",
    ] {
        assert!(
            benchmark.contains(field),
            "bootstrap benchmark must report `{field}`"
        );
    }
    assert!(benchmark.contains("BootstrapPreciseStopTheWorld"));
    let relocation_benchmark = read_required(runtime.join("collector/benches/relocation.rs"));
    for required in [
        "pop-runtime-benchmark-v1",
        "collector_stage=RelocationConformance",
        "relocated_roots",
        "nanoseconds_per_operation",
    ] {
        assert!(
            relocation_benchmark.contains(required),
            "relocation benchmark must report `{required}`"
        );
    }
}

fn assert_runtime_consumer_dependencies(root: &Path) {
    let backend_api = read_required(root.join("crates/compiler/backend-api/src/lib.rs"));
    for required in [
        "RuntimeProfile",
        "BackendGcCapability",
        "RelocatingManagedReferences",
        "validate_runtime_profile",
        "IncompatibleNativeAbi",
    ] {
        assert!(
            backend_api.contains(required),
            "backend API must enforce runtime-profile fact `{required}`"
        );
    }

    let internal_manifest = read_required(root.join("crates/libraries/internal/Cargo.toml"));
    assert!(internal_manifest.contains("pop-runtime-native-abi.workspace = true"));
    assert!(!internal_manifest.contains("pop-runtime-collector.workspace = true"));
    assert!(!internal_manifest.contains("pop-runtime-native.workspace = true"));

    let llvm_manifest = read_required(root.join("crates/compiler/backends/llvm/Cargo.toml"));
    assert!(llvm_manifest.contains("pop-runtime-native-abi.workspace = true"));
    assert!(!llvm_manifest.contains("pop-runtime-collector.workspace = true"));

    let interpreter_manifest =
        read_required(root.join("crates/compiler/backends/mir-interp/Cargo.toml"));
    assert!(interpreter_manifest.contains("pop-runtime-collector.workspace = true"));
    assert!(!interpreter_manifest.contains("pop-runtime-native.workspace = true"));
}

fn assert_runtime_documentation(runtime: &Path) {
    for crate_directory in ["interface", "collector", "native-abi", "native"] {
        let crate_root = runtime.join(crate_directory);
        assert!(
            crate_root.join("README.md").is_file(),
            "runtime ownership crate `{crate_directory}` needs a README"
        );
        let source = read_required(crate_root.join("src/lib.rs"));
        assert!(
            source.lines().count() <= 80,
            "runtime crate root `{crate_directory}` must remain a thin module inventory"
        );
    }

    let guide = read_required(runtime.join("README.md"));
    for required in [
        "## Dependency direction",
        "## Choose the owning crate",
        "pop-runtime-interface",
        "pop-runtime-collector",
        "pop-runtime-native-abi",
        "pop-runtime-native",
        "Performance",
    ] {
        assert!(
            guide.contains(required),
            "runtime contributor guide must explain `{required}`"
        );
    }
}

fn read_required(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn read_rust_sources(directory: &Path) -> String {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .map(|entry| entry.expect("read runtime source entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(read_required)
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_runtime_modules_are_focused(directory: &Path, maximum_lines: usize) {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .map(|entry| entry.expect("read runtime source entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    paths.sort();

    for path in paths {
        let source = read_required(&path);
        assert!(
            source.lines().count() <= maximum_lines,
            "runtime responsibility module {} exceeds {maximum_lines} lines",
            path.display()
        );
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
fn foundation_libraries_are_partitioned_by_contributor_ownership() {
    let root = repository_root();
    let standard = root.join("crates/libraries/standard");
    let internal = root.join("crates/libraries/internal");

    let standard_root =
        fs::read_to_string(standard.join("src/lib.rs")).expect("read pop-standard crate root");
    for declaration in ["mod native_output;", "pub mod text;"] {
        assert!(
            standard_root.lines().any(|line| line.trim() == declaration),
            "pop-standard must explicitly inventory `{declaration}`"
        );
    }
    for implementation in ["pub fn ", "pub enum ", "extern \"C\" fn", "pub mod math {"] {
        assert!(
            !standard_root.contains(implementation),
            "pop-standard src/lib.rs must remain a thin module inventory"
        );
    }

    let internal_root =
        fs::read_to_string(internal.join("src/lib.rs")).expect("read pop-internal crate root");
    assert!(
        internal_root
            .lines()
            .any(|line| line.trim() == "pub mod runtime;")
    );
    assert!(!internal_root.contains("pub mod runtime {"));

    for relative in [
        "standard/src/native_output.rs",
        "standard/src/text.rs",
        "standard/tests/native_output.rs",
        "standard/tests/text.rs",
        "standard/pop/bubble.toml",
        "standard/pop/src/lib.pop",
        "standard/pop/src/math.pop",
        "standard/pop/src/sequence.pop",
        "internal/src/runtime.rs",
        "internal/tests/runtime.rs",
        "internal/pop/bubble.toml",
        "internal/pop/src/lib.pop",
    ] {
        assert!(
            root.join("crates/libraries").join(relative).is_file(),
            "foundation library ownership file `{relative}` is missing"
        );
    }

    let contributor_guide = fs::read_to_string(root.join("crates/libraries/README.md"))
        .expect("read foundation-library contributor guide");
    for required in [
        "ordinary portable library function",
        "compiler-known identity",
        "trusted intrinsic",
        "native ABI",
        "`Pop.Standard` is the recommended contribution path",
        "## Choose the right contribution path",
        "## Step-by-step: add a `Pop.Standard` function",
        "## Before proposing `Pop.Internal` work",
        "## Pull request checklist",
    ] {
        assert!(
            contributor_guide.contains(required),
            "foundation-library contributor guide must explain `{required}` changes"
        );
    }
}

#[test]
fn foundation_native_adapters_use_typed_poplib_descriptors() {
    let root = repository_root();
    let bridge = root.join("crates/libraries/bridge");
    let macros = root.join("crates/libraries/macros");

    for path in [
        bridge.join("Cargo.toml"),
        bridge.join("src/lib.rs"),
        macros.join("Cargo.toml"),
        macros.join("src/lib.rs"),
    ] {
        assert!(
            path.is_file(),
            "ADR 0037 support file is missing: {}",
            path.display()
        );
    }

    let bridge_manifest =
        fs::read_to_string(bridge.join("Cargo.toml")).expect("read pop-library-bridge manifest");
    let macro_manifest =
        fs::read_to_string(macros.join("Cargo.toml")).expect("read pop-library-macros manifest");
    let macro_source =
        fs::read_to_string(macros.join("src/lib.rs")).expect("read pop-library-macros source");
    assert!(bridge_manifest.contains("pop-library-macros.workspace = true"));
    assert!(!macro_manifest.contains("syn ="));
    assert!(!macro_manifest.contains("quote ="));

    let standard = fs::read_to_string(root.join("crates/libraries/standard/src/native_output.rs"))
        .expect("read native standard adapters");
    assert_eq!(standard.matches("#[poplib(").count(), 2);
    assert!(!standard.contains("#[unsafe(no_mangle)]"));
    assert!(standard.contains("pub const NATIVE_EXPORTS"));
    assert!(standard.contains("POP_STD_PRINT_INT_POPLIB_EXPORT"));
    assert!(standard.contains("POP_STD_PRINT_STRING_POPLIB_EXPORT"));

    let internal = fs::read_to_string(root.join("crates/libraries/internal/src/lib.rs"))
        .expect("read internal foundation root");
    assert!(internal.contains("pub use runtime::NATIVE_EXPORTS;"));

    for forbidden in ["inventory::", "linkme::", "ctor::", "std::fs::read_dir"] {
        assert!(!standard.contains(forbidden));
        assert!(!internal.contains(forbidden));
        assert!(!macro_source.contains(forbidden));
    }
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
fn portable_sequence_algorithms_have_one_pop_implementation() {
    let root = repository_root();
    let standard = fs::read_to_string(root.join("crates/libraries/standard/src/lib.rs"))
        .expect("read Pop.Standard Rust module inventory");
    let sequence = fs::read_to_string(root.join("crates/libraries/standard/pop/src/sequence.pop"))
        .expect("read Pop.Sequence source");

    assert!(!standard.contains("pub mod sequence;"));
    assert!(
        !root
            .join("crates/libraries/standard/src/sequence.rs")
            .exists()
    );
    for function in [
        "map",
        "filter",
        "fold",
        "collect",
        "any",
        "all",
        "count",
        "isEmpty",
        "firstOr",
        "lastOr",
        "each",
        "none",
        "countWhere",
        "take",
        "drop",
        "takeWhile",
        "dropWhile",
        "concat",
        "sum",
        "product",
        "minOr",
        "maxOr",
        "findOr",
        "indexOr",
        "sumBy",
        "productBy",
        "minByOr",
        "maxByOr",
        "append",
        "prepend",
        "scan",
        "elementAtOr",
        "findLastOr",
        "indexLastOr",
        "reduceOr",
    ] {
        assert!(
            sequence.contains(&format!("public function {function}<")),
            "Pop.Sequence must own `{function}` as ordinary Pop source"
        );
    }
}

#[test]
fn portable_integer_math_has_one_pop_implementation() {
    let root = repository_root();
    let standard = fs::read_to_string(root.join("crates/libraries/standard/src/lib.rs"))
        .expect("read Pop.Standard Rust module inventory");
    let math_path = root.join("crates/libraries/standard/pop/src/math.pop");

    assert!(!standard.contains("pub mod math;"));
    assert!(!root.join("crates/libraries/standard/src/math.rs").exists());
    let math = fs::read_to_string(&math_path).expect("read Pop.Math source");
    for function in ["min", "max", "abs", "gcd", "sign", "lcm", "coprime"] {
        assert!(
            math.contains(&format!("public function {function}(")),
            "Pop.Math must own `{function}` as ordinary Pop source"
        );
    }
}

#[test]
fn standard_bootstrap_preserves_the_adr_0058_prelude() {
    let path = repository_root().join("libraries/standard/bootstrap/prelude-types.tsv");
    let metadata = fs::read_to_string(&path).expect("read Standard prelude metadata");
    let rows: Vec<_> = metadata
        .lines()
        .skip(2)
        .filter(|line| !line.is_empty())
        .collect();

    assert_eq!(
        rows,
        vec![
            "100\tResult\tPop.Standard\t2\tNominal\ttrue",
            "101\tList\tPop.Standard\t1\tNominal\ttrue",
            "102\tSet\tPop.Standard\t1\tNominal\ttrue",
            "103\tRange\tPop.Standard\t1\tNominal\ttrue",
            "104\tTask\tPop.Standard\t1\tNominal\ttrue",
            "105\tGuid\tPop.Standard\t0\tNominal\ttrue",
            "106\tIterable\tPop.Standard\t1\tInterface\ttrue",
            "107\tIterator\tPop.Standard\t1\tInterface\ttrue",
            "108\tEqual\tPop.Standard\t1\tInterface\ttrue",
            "109\tOrder\tPop.Standard\t1\tInterface\ttrue",
            "110\tHash\tPop.Standard\t1\tInterface\ttrue",
            "111\tClose\tPop.Standard\t0\tInterface\ttrue",
            "112\tAsyncClose\tPop.Standard\t0\tInterface\ttrue",
            "113\tIteration\tPop.Standard\t1\tNominal\ttrue",
            "114\tCancelToken\tPop.Standard\t0\tNominal\ttrue",
        ],
        "ADR 0058 prelude inventory drifted"
    );
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
    let (actual, inventory_tiers, _, inventory) = parse_root_inventory(&index);
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

#[test]
fn public_library_root_manifest_matches_the_authoritative_catalog() {
    let root = repository_root();
    let architecture =
        read_required(root.join("architecture/22-public-standard-library-architecture.md"));
    let (_, tiers, catalogs, _) = parse_root_inventory(&architecture);
    let manifest = read_required(root.join("libraries/catalog/public-roots.tsv"));
    let mut lines = manifest.lines();
    assert_eq!(lines.next(), Some("schemaVersion\t1"));
    assert_eq!(lines.next(), Some("publicRoot\ttier\tcatalog\tstatus"));

    let allowed_catalogs = [
        "application/media/science",
        "core/portable",
        "data/observability/tooling",
        "system/network/security",
    ];
    let allowed_statuses = ["implemented", "bootstrap", "prototype", "planned"];
    let mut roots = BTreeSet::new();
    let mut statuses = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let columns: Vec<_> = line.split('\t').collect();
        assert_eq!(columns.len(), 4, "invalid public root manifest row: {line}");
        let [public_root, tier, catalog, status] = columns.as_slice() else {
            unreachable!("column count checked above")
        };
        assert!(
            roots.insert((*public_root).to_owned()),
            "duplicate `{public_root}`"
        );
        assert_eq!(tiers.get(*public_root).map(String::as_str), Some(*tier));
        assert!(
            allowed_catalogs.contains(catalog),
            "unknown catalog `{catalog}`"
        );
        assert_eq!(
            catalogs.get(*public_root).map(String::as_str),
            Some(*catalog),
            "owning catalog drifted for `{public_root}`"
        );
        assert!(
            allowed_statuses.contains(status),
            "unknown status `{status}`"
        );
        assert!(
            statuses
                .insert((*public_root).to_owned(), (*status).to_owned())
                .is_none(),
            "duplicate status for `{public_root}`"
        );
    }
    assert_eq!(
        roots,
        PUBLIC_LIBRARY_ROOTS
            .iter()
            .map(|root| (*root).to_owned())
            .collect(),
        "machine-readable root manifest drifted"
    );

    let bootstrap: BTreeSet<_> = [
        "Ai", "Cli", "Command", "Data", "Lsp", "Rpc", "Settings", "Source", "Sql", "Store",
        "Syntax",
    ]
    .into_iter()
    .collect();
    for public_root in PUBLIC_LIBRARY_ROOTS {
        let expected = if ["Math", "Sequence", "Text"].contains(public_root) {
            "prototype"
        } else if bootstrap.contains(public_root) {
            "bootstrap"
        } else {
            "planned"
        };
        assert_eq!(
            statuses.get(*public_root).map(String::as_str),
            Some(expected),
            "status drifted for `{public_root}`"
        );
    }
}

fn parse_root_inventory(
    index: &str,
) -> (
    BTreeSet<String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    &str,
) {
    let start = index
        .find("<!-- namespace-roots:start -->")
        .expect("namespace root inventory start marker");
    let end = index
        .find("<!-- namespace-roots:end -->")
        .expect("namespace root inventory end marker");
    let inventory = &index[start..end];

    let mut actual = BTreeSet::new();
    let mut inventory_tiers = BTreeMap::new();
    let mut inventory_catalogs = BTreeMap::new();
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
        assert!(
            inventory_catalogs
                .insert(name.to_owned(), columns[3].to_owned())
                .is_none(),
            "duplicate catalog assignment for `{name}`"
        );
    }
    (actual, inventory_tiers, inventory_catalogs, inventory)
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
    assert!(
        examples
            .contains("closures, `Result`, exhaustive `match`, prefix `try`, and lexical `defer`")
    );
    assert!(examples.contains("Postfix `?` remains optional-only."));
    assert!(examples.contains("Proposed syntax only."));
    for stale in ["Data.Json", "Text.Pattern", "Async.run", "Io.open"] {
        assert!(
            !examples.contains(stale),
            "examples retain stale API `{stale}`"
        );
    }
}
