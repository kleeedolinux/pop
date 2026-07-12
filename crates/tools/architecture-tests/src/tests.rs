//! Conformance tests for ADR 0018 and architecture section 11.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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

        for inherited in [
            "version.workspace = true",
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
fn driver_declares_the_pop_binary() {
    let root = repository_root();
    let manifest_path = root.join("crates/compiler/driver/Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("read pop-driver manifest");

    assert!(manifest.contains("name = \"pop\""));
    assert!(manifest.contains("path = \"src/main.rs\""));
}
