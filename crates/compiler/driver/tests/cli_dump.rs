use std::path::PathBuf;
use std::process::{Command, Output};

use pop_driver::load_poplib;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("cli")
        .join(name)
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("driver crate is under repository root")
        .join("examples")
        .join(name)
}

fn bpf_example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("driver crate is under repository root")
        .join("examples")
        .join("bpf")
        .join(name)
}

fn run_pop(arguments: &[&str], source: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pop"));
    command.args(arguments);
    if let Some(source) = source {
        command.arg(fixture(source));
    }
    command.output().expect("pop command runs")
}

fn output_text(output: &[u8]) -> String {
    String::from_utf8(output.to_vec()).expect("pop output is UTF-8")
}

fn temporary_package(name: &str, library: &str, binary: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("pop-package-{name}-{}", std::process::id()));
    let source = root.join("src");
    std::fs::create_dir_all(&source).expect("create temporary Package");
    std::fs::write(
        root.join("bubble.toml"),
        "[package]\nname = \"Studio.Entry\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .expect("write Package manifest");
    std::fs::write(source.join("lib.pop"), library).expect("write library Bubble root");
    std::fs::write(source.join("main.pop"), binary).expect("write binary Bubble root");
    root
}

#[test]
fn check_dumps_deterministic_verified_hir_for_a_pop_module() {
    let first = run_check_dump("inspectable.pop", "hir");
    let second = run_check_dump("inspectable.pop", "hir");

    assert!(
        first.status.success(),
        "stderr:\n{}",
        output_text(&first.stderr)
    );
    assert_eq!(
        first.stdout, second.stdout,
        "HIR dump must be deterministic"
    );
    assert_eq!(first.stderr, second.stderr);

    let stdout = output_text(&first.stdout);
    assert!(stdout.starts_with("hir bubble b0 namespace n0\n"));
    assert!(stdout.contains("function s0 f0 public m0 b0 add("));
    assert!(!stdout.contains("mir bubble"));
    assert!(!stdout.to_ascii_lowercase().contains("dynamic"));
    assert!(!stdout.to_ascii_lowercase().contains("llvm"));
    assert!(first.stderr.is_empty());
}

#[test]
fn check_dumps_deterministic_verified_canonical_mir_for_a_pop_module() {
    let first = run_check_dump("inspectable.pop", "mir");
    let second = run_check_dump("inspectable.pop", "mir");

    assert!(
        first.status.success(),
        "stderr:\n{}",
        output_text(&first.stderr)
    );
    assert_eq!(
        first.stdout, second.stdout,
        "MIR dump must be deterministic"
    );
    assert_eq!(first.stderr, second.stderr);

    let stdout = output_text(&first.stdout);
    assert!(stdout.starts_with("mir bubble b0 namespace n0\n"));
    assert!(stdout.contains("integer.checkedAdd Int64"));
    assert!(!stdout.contains("hir bubble"));
    assert!(!stdout.to_ascii_lowercase().contains("dynamic"));
    assert!(!stdout.to_ascii_lowercase().contains("llvm"));
    assert!(first.stderr.is_empty());
}

#[test]
fn check_dumps_deterministic_verified_llvm_ir_for_a_pop_module() {
    let first = run_check_dump("inspectable.pop", "ll");
    let second = run_check_dump("inspectable.pop", "ll");

    assert!(
        first.status.success(),
        "stderr:\n{}",
        output_text(&first.stderr)
    );
    assert_eq!(
        first.stdout, second.stdout,
        "LLVM IR dump must be deterministic"
    );
    assert_eq!(first.stderr, second.stderr);

    let stdout = output_text(&first.stdout);
    assert!(stdout.starts_with("; Pop Lang native module\n"));
    assert!(stdout.contains("target triple = \"x86_64-unknown-linux-gnu\""));
    assert!(stdout.contains("define i64 @pop_b0_s0(i64 %v0, i64 %v1)"));
    assert!(stdout.contains("@llvm.sadd.with.overflow.i64"));
    assert!(!stdout.contains("hir bubble"));
    assert!(!stdout.contains("mir bubble"));
    assert!(first.stderr.is_empty());
}

#[test]
fn check_accepts_repeatable_dump_options_in_command_line_order() {
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(fixture("inspectable.pop"))
        .args(["--dump", "hir", "--dump", "mir", "--dump", "ll"])
        .output()
        .expect("pop command runs");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        output_text(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let stdout = output_text(&output.stdout);
    let hir = stdout.find("hir bubble").expect("HIR dump");
    let mir = stdout.find("mir bubble").expect("MIR dump");
    let llvm = stdout.find("; Pop Lang native module").expect("LLVM dump");
    assert!(
        hir < mir && mir < llvm,
        "requested dump order must be preserved"
    );
    assert_eq!(stdout.matches("hir bubble").count(), 1);
    assert_eq!(stdout.matches("mir bubble").count(), 1);
    assert_eq!(stdout.matches("; Pop Lang native module").count(), 1);
}

#[test]
fn invalid_source_emits_a_structured_diagnostic_and_no_dump() {
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(fixture("invalid.pop"))
        .args(["--dump", "hir", "--dump", "mir", "--dump", "ll"])
        .output()
        .expect("pop command runs");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "invalid HIR/MIR/LLVM must not be dumped"
    );

    let stderr = output_text(&output.stderr);
    assert!(
        stderr.lines().any(|line| line.starts_with("POP1002@")),
        "stderr must contain the stable diagnostic code and span: {stderr:?}"
    );
}

#[test]
fn unsupported_dump_kind_is_a_usage_error() {
    let output = run_check_dump("inspectable.pop", "llvm");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("hir|mir|ll"));
}

#[test]
fn missing_check_arguments_are_a_usage_error() {
    let output = run_pop(&["check"], None);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("pop check"));
}

#[test]
fn transpile_to_c_is_deterministic_and_emits_a_complete_translation_unit() {
    let source = fixture("cTranspile.pop");
    let first = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["transpile"])
        .arg(&source)
        .args(["--to", "c"])
        .output()
        .expect("pop transpile runs");
    let second = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["transpile"])
        .arg(&source)
        .args(["--to", "c"])
        .output()
        .expect("pop transpile runs deterministically");

    assert!(
        first.status.success(),
        "stderr:\n{}",
        output_text(&first.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    assert!(first.stderr.is_empty());
    let c = output_text(&first.stdout);
    assert!(c.starts_with("/* Generated by Pop Lang"));
    assert!(c.contains("int main(void)"));
    assert!(!c.contains("answer"));
}

#[test]
fn transpile_rejects_unknown_targets_and_runtime_features_without_partial_c() {
    let unknown = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["transpile"])
        .arg(fixture("cTranspile.pop"))
        .args(["--to", "javascript"])
        .output()
        .expect("pop transpile usage error");
    assert_eq!(unknown.status.code(), Some(2));
    assert!(unknown.stdout.is_empty());
    assert!(output_text(&unknown.stderr).contains("expected c"));

    let unsupported = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["transpile"])
        .arg(example("nativeClass.pop"))
        .args(["--to", "c"])
        .output()
        .expect("pop transpile capability error");
    assert!(!unsupported.status.success());
    assert!(unsupported.stdout.is_empty());
    assert!(output_text(&unsupported.stderr).contains("requires the Pop runtime"));
}

#[test]
fn transpile_supports_the_runtime_free_native_math_example() {
    let name = "nativeMath.pop";
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["transpile"])
        .arg(example(name))
        .args(["--to", "c"])
        .output()
        .expect("pop transpile example");
    assert!(
        output.status.success(),
        "{name}: {}",
        output_text(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(output_text(&output.stdout).contains("int main(void)"));
}

#[test]
fn bpf_build_requires_a_known_explicit_target() {
    let object = std::env::temp_dir().join(format!("pop-bpf-unknown-{}.o", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["build"])
        .arg(bpf_example("xdpPass.pop"))
        .args([
            "--target",
            "bpf-unknown-linux",
            "--runtime-profile",
            "linux-ebpf",
            "--bpf-program",
            "xdp",
            "--emit-object",
        ])
        .arg(&object)
        .output()
        .expect("pop build bpf usage runs");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!object.exists(), "failed BPF build must not emit an object");
    assert!(output_text(&output.stderr).contains("unknown Pop Lang target triple"));
}

#[test]
fn bpf_build_rejects_unknown_runtime_profile_before_artifact_emission() {
    let object = std::env::temp_dir().join(format!("pop-bpf-profile-{}.o", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["build"])
        .arg(bpf_example("xdpPass.pop"))
        .args([
            "--target",
            "bpfel-unknown-none",
            "--runtime-profile",
            "kernel-default",
            "--bpf-program",
            "xdp",
            "--emit-object",
        ])
        .arg(&object)
        .output()
        .expect("pop build bpf usage runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!object.exists(), "failed BPF build must not emit an object");
    assert!(output_text(&output.stderr).contains("unknown runtime profile"));
}

#[test]
fn transpile_rejects_the_looping_print_example_without_a_runtime_fallback() {
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["transpile"])
        .arg(example("nativePrint.pop"))
        .args(["--to", "c"])
        .output()
        .expect("pop transpile capability error");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("unsupported MIR instruction"));
}

#[test]
fn build_and_run_emit_and_execute_a_native_pop_program_with_standard_output() {
    let output_path = std::env::temp_dir().join("pop-native-cli-example");
    let build = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("build")
        .arg(fixture("native.pop"))
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("pop build runs");
    assert!(
        build.status.success(),
        "stderr:\n{}",
        output_text(&build.stderr)
    );
    assert!(output_path.is_file(), "pop build must emit an executable");

    let executable = Command::new(&output_path)
        .args(["first", "", "Pop 🫧"])
        .output()
        .expect("built Pop executable runs");
    assert!(executable.status.success());
    assert_eq!(
        output_text(&executable.stdout),
        "typed helper result\n\nPop 🫧\n42\n"
    );

    let run = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("run")
        .arg(fixture("native.pop"))
        .args(["--", "first", "", "Pop 🫧"])
        .output()
        .expect("pop run executes with program arguments");
    assert!(
        run.status.success(),
        "stderr:\n{}",
        output_text(&run.stderr)
    );
    assert_eq!(
        output_text(&run.stdout),
        "typed helper result\n\nPop 🫧\n42\n"
    );

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;

        let invalid = Command::new(&output_path)
            .arg(std::ffi::OsString::from_vec(vec![0xff]))
            .output()
            .expect("native program receives invalid platform bytes");
        assert!(
            !invalid.status.success(),
            "invalid UTF-8 must trap before user main"
        );
        assert!(invalid.stdout.is_empty());
    }
    let _ = std::fs::remove_file(output_path);
}

#[test]
fn native_build_rejects_every_noncanonical_entry_shape() {
    for fixture_name in [
        "nativeMissingMain.pop",
        "nativePublicMain.pop",
        "nativeWrongParameters.pop",
        "nativeWrongResult.pop",
    ] {
        let output_path = std::env::temp_dir().join(format!("pop-invalid-entry-{fixture_name}"));
        let output = Command::new(env!("CARGO_BIN_EXE_pop"))
            .arg("build")
            .arg(fixture(fixture_name))
            .arg("--output")
            .arg(&output_path)
            .output()
            .expect("pop build runs");
        assert!(
            !output.status.success(),
            "{fixture_name} unexpectedly built"
        );
        let stderr = output_text(&output.stderr);
        assert!(
            stderr.contains("binary entry must be"),
            "{fixture_name} emitted an imprecise entry diagnostic: {stderr}"
        );
        assert!(!output_path.exists());
    }
}

#[test]
fn clean_main_forms_print_and_complete_without_return_zero() {
    for fixture_name in ["nativeCleanMain.pop", "nativePrivateCleanMain.pop"] {
        let output_path = std::env::temp_dir().join(format!("pop-clean-main-{fixture_name}"));
        let build = Command::new(env!("CARGO_BIN_EXE_pop"))
            .arg("build")
            .arg(fixture(fixture_name))
            .arg("--output")
            .arg(&output_path)
            .output()
            .expect("pop build runs");
        assert!(
            build.status.success(),
            "{fixture_name} failed: {}",
            output_text(&build.stderr)
        );
        let run = Command::new(&output_path)
            .output()
            .expect("clean main executable runs");
        assert!(run.status.success());
        assert_eq!(output_text(&run.stdout), "42\n");

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;

            let invalid = Command::new(&output_path)
                .arg(std::ffi::OsString::from_vec(vec![0xff]))
                .output()
                .expect("no-argument main ignores platform argument encoding");
            assert!(invalid.status.success());
            assert_eq!(output_text(&invalid.stdout), "42\n");
        }
        let _ = std::fs::remove_file(output_path);
    }
}

#[test]
fn package_run_compiles_an_internal_library_without_main() {
    let package = temporary_package(
        "internal-library",
        "namespace Studio.Entry.Library\n\
         public function announce()\n\
             print(41)\n\
         end\n\
         public function message(): String\n\
             return \"library\"\n\
         end\n",
        "namespace Studio.Entry.Application\n\
         function main()\n\
             Studio.Entry.Library.announce()\n\
             print(42)\n\
         end\n",
    );
    let manifest = package.join("bubble.toml");
    let run = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["run", "--manifestPath"])
        .arg(&manifest)
        .args(["--", "argument"])
        .output()
        .expect("pop run executes a Package binary");
    assert!(
        run.status.success(),
        "Package run failed: {}",
        output_text(&run.stderr)
    );
    assert_eq!(output_text(&run.stdout), "41\n42\n");
    std::fs::remove_dir_all(package).expect("remove temporary Package");
}

#[test]
fn package_check_and_build_use_manifest_selected_bubbles() {
    let package = temporary_package(
        "check-build",
        "namespace Studio.Entry.Library\n\
         public function answer(): Int\n\
             return 42\n\
         end\n",
        "namespace Studio.Entry.Application\n\
         function main(): Int\n\
             return Studio.Entry.Library.answer()\n\
         end\n",
    );
    let manifest = package.join("bubble.toml");

    let check = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(&manifest)
        .output()
        .expect("pop check resolves a Package");
    assert!(
        check.status.success(),
        "Package check failed: {}",
        output_text(&check.stderr)
    );
    assert!(check.stdout.is_empty());
    assert!(
        !package.join("target").exists(),
        "check must not emit artifacts"
    );

    let build = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["build", "--manifestPath"])
        .arg(&manifest)
        .output()
        .expect("pop build resolves a Package");
    assert!(
        build.status.success(),
        "Package build failed: {}",
        output_text(&build.stderr)
    );
    let executable = package.join("target/debug/Studio.Entry");
    assert!(executable.is_file());
    let artifact = package.join("target/debug/deps/Studio.Entry.poplib");
    let loaded = load_poplib(&artifact).expect("build emits one verified library artifact");
    assert_eq!(loaded.package(), "Studio.Entry");
    assert_eq!(loaded.version(), "0.1.0");
    assert_eq!(loaded.bubble(), "Studio.Entry");
    assert!(loaded.documentation().is_some());
    assert_eq!(
        loaded.target_implementation().map(|(target, _)| target),
        Some("x86_64-unknown-linux-gnu")
    );
    let status = Command::new(executable)
        .status()
        .expect("manifest-built executable runs");
    assert_eq!(status.code(), Some(42));

    std::fs::remove_dir_all(package).expect("remove temporary Package");
}

#[test]
fn package_run_resolves_and_links_exact_local_path_dependencies() {
    let workspace =
        std::env::temp_dir().join(format!("pop-local-dependencies-{}", std::process::id()));
    let data = workspace.join("data");
    let application = workspace.join("application");
    std::fs::create_dir_all(data.join("src")).expect("create dependency Package");
    std::fs::create_dir_all(application.join("src")).expect("create application Package");
    std::fs::write(
        data.join("bubble.toml"),
        "[package]\nname = \"Studio.Data\"\nversion = \"2.1.0\"\nedition = \"2026\"\n",
    )
    .expect("write dependency manifest");
    std::fs::write(
        data.join("src/lib.pop"),
        "namespace Studio.Data\n\
         public function identity(value: Int): Int\n\
             return value\n\
         end\n",
    )
    .expect("write dependency library");
    std::fs::write(
        application.join("bubble.toml"),
        "[package]\n\
         name = \"Studio.Application\"\n\
         version = \"0.1.0\"\n\
         edition = \"2026\"\n\
         [dependencies]\n\
         StudioData = { path = \"../data\", version = \"2.1.0\", bubble = \"Studio.Data\" }\n",
    )
    .expect("write application manifest");
    std::fs::write(
        application.join("src/lib.pop"),
        "namespace Studio.Application\n\
         public function dependencyIdentity(value: Int): Int\n\
             return Studio.Data.identity(value)\n\
         end\n",
    )
    .expect("write application library");
    std::fs::write(
        application.join("src/main.pop"),
        "namespace Studio.Application\n\
         function main()\n\
             print(Studio.Application.dependencyIdentity(41))\n\
             print(42)\n\
         end\n",
    )
    .expect("write application binary");

    let run = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["run", "--manifestPath"])
        .arg(application.join("bubble.toml"))
        .output()
        .expect("pop run resolves local Package dependency");
    assert!(
        run.status.success(),
        "local dependency run failed: {}",
        output_text(&run.stderr)
    );
    assert_eq!(output_text(&run.stdout), "41\n42\n");
    let dependency_artifact = application.join("target/debug/deps/Studio.Data.poplib");
    let loaded = load_poplib(&dependency_artifact)
        .expect("dependency build emits its verified library artifact");
    assert_eq!(loaded.package(), "Studio.Data");
    assert_eq!(loaded.version(), "2.1.0");
    assert_eq!(loaded.bubble(), "Studio.Data");
    assert!(loaded.dependencies().is_empty());
    let application_artifact = application.join("target/debug/deps/Studio.Application.poplib");
    let loaded = load_poplib(&application_artifact)
        .expect("root library artifact records its exact dependency");
    let [dependency] = loaded.dependencies() else {
        panic!("one exact artifact dependency");
    };
    assert_eq!(dependency.package(), "Studio.Data");
    assert_eq!(dependency.version(), "2.1.0");
    assert_eq!(dependency.bubble(), "Studio.Data");

    std::fs::remove_dir_all(workspace).expect("remove temporary Workspace");
}

#[test]
fn package_check_rejects_local_dependency_cycles_before_analysis() {
    let workspace = std::env::temp_dir().join(format!("pop-local-cycle-{}", std::process::id()));
    for (directory, name, dependency, namespace) in [
        ("first", "Studio.First", "../second", "Studio.First"),
        ("second", "Studio.Second", "../first", "Studio.Second"),
    ] {
        let package = workspace.join(directory);
        std::fs::create_dir_all(package.join("src")).expect("create cyclic Package");
        std::fs::write(
            package.join("bubble.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2026\"\n\
                 [dependencies]\nOther = {{ path = \"{dependency}\", version = \"1.0.0\" }}\n"
            ),
        )
        .expect("write cyclic manifest");
        std::fs::write(
            package.join("src/lib.pop"),
            format!("namespace {namespace}\npublic function value(): Int\n    return 1\nend\n"),
        )
        .expect("write cyclic library");
    }

    let check = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(workspace.join("first/bubble.toml"))
        .output()
        .expect("pop check detects dependency cycle");
    assert!(!check.status.success());
    assert!(
        output_text(&check.stderr).contains("Package dependency cycle"),
        "cycle error was not precise: {}",
        output_text(&check.stderr)
    );
    assert!(!workspace.join("first/target").exists());

    std::fs::remove_dir_all(workspace).expect("remove temporary Workspace");
}

#[test]
fn virtual_workspace_uses_default_members_and_one_shared_target_root() {
    let workspace =
        std::env::temp_dir().join(format!("pop-virtual-workspace-{}", std::process::id()));
    let application = workspace.join("packages/application");
    let ignored = workspace.join("packages/ignored");
    std::fs::create_dir_all(application.join("src")).expect("create default member");
    std::fs::create_dir_all(ignored.join("src")).expect("create non-default member");
    std::fs::write(
        workspace.join("bubble.toml"),
        "[workspace]\n\
         members = [\"packages/*\"]\n\
         defaultMembers = [\"packages/application\"]\n\
         resolver = \"1\"\n",
    )
    .expect("write Workspace manifest");
    for (root, name, value) in [
        (&application, "Studio.Application", 42),
        (&ignored, "Studio.Ignored", 99),
    ] {
        std::fs::write(
            root.join("bubble.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n"),
        )
        .expect("write member manifest");
        std::fs::write(
            root.join("src/main.pop"),
            format!("namespace {name}\nfunction main(): Int\n    return {value}\nend\n"),
        )
        .expect("write member binary");
    }

    let manifest = workspace.join("bubble.toml");
    let check = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(&manifest)
        .output()
        .expect("pop check selects Workspace defaults");
    assert!(
        check.status.success(),
        "Workspace check failed: {}",
        output_text(&check.stderr)
    );
    assert!(!workspace.join("target").exists());

    let build = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["build", "--manifestPath"])
        .arg(&manifest)
        .output()
        .expect("pop build selects Workspace defaults");
    assert!(
        build.status.success(),
        "Workspace build failed: {}",
        output_text(&build.stderr)
    );
    let executable = workspace.join("target/debug/Studio.Application");
    assert!(executable.is_file());
    assert!(!workspace.join("target/debug/Studio.Ignored").exists());
    assert!(!application.join("target").exists());
    assert_eq!(
        Command::new(executable)
            .status()
            .expect("Workspace executable runs")
            .code(),
        Some(42)
    );

    std::fs::remove_dir_all(workspace).expect("remove temporary Workspace");
}

#[test]
fn package_documentation_emits_checked_public_xml_separately() {
    let package = temporary_package(
        "documentation",
        "namespace Studio.Entry.Library\n\
         --- <summary>\n\
         --- Returns the answer.\n\
         --- </summary>\n\
         ---\n\
         --- <returns>\n\
         --- The stable answer.\n\
         --- </returns>\n\
         public function answer(): Int\n\
             return 42\n\
         end\n",
        "namespace Studio.Entry.Application\n\
         function main(): Int\n\
             return Studio.Entry.Library.answer()\n\
         end\n",
    );
    let documentation = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["documentation", "--manifestPath"])
        .arg(package.join("bubble.toml"))
        .output()
        .expect("pop documentation resolves a Package");
    assert!(
        documentation.status.success(),
        "documentation failed: {}",
        output_text(&documentation.stderr)
    );
    let output = package.join("target/documentation/Studio.Entry/documentation.xml");
    let xml = std::fs::read_to_string(output).expect("documentation.xml");
    assert!(xml.contains("schemaVersion=\"1\" bubble=\"Studio.Entry\""));
    assert!(xml.contains("id=\"function:Studio.Entry.Library.answer()\""));
    assert!(xml.contains("<summary>Returns the answer.</summary>"));
    assert!(
        !xml.contains("main"),
        "private binary docs must not be emitted"
    );

    std::fs::remove_dir_all(package).expect("remove temporary Package");
}

#[test]
fn manifest_commands_write_one_deterministic_lock_and_enforce_locked_modes() {
    let package = temporary_package(
        "lock-policy",
        "namespace Studio.Entry.Library\n\
         public function answer(): Int\n\
             return 42\n\
         end\n",
        "namespace Studio.Entry.Application\n\
         function main(): Int\n\
             return Studio.Entry.Library.answer()\n\
         end\n",
    );
    let manifest = package.join("bubble.toml");
    let first = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(&manifest)
        .output()
        .expect("initial lock generation");
    assert!(first.status.success(), "{}", output_text(&first.stderr));
    let lock_path = package.join("bubble.lock");
    let first_bytes = std::fs::read(&lock_path).expect("generated bubble.lock");
    let second = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(&manifest)
        .output()
        .expect("repeat lock generation");
    assert!(second.status.success(), "{}", output_text(&second.stderr));
    assert_eq!(std::fs::read(&lock_path).expect("stable lock"), first_bytes);

    let locked = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(&manifest)
        .arg("--locked")
        .output()
        .expect("locked check");
    assert!(locked.status.success(), "{}", output_text(&locked.stderr));

    std::fs::write(
        package.join("src/lib.pop"),
        "namespace Studio.Entry.Library\npublic function answer(): Int\n    return 43\nend\n",
    )
    .expect("change locked input");
    let changed = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(&manifest)
        .arg("--locked")
        .output()
        .expect("locked change rejection");
    assert!(!changed.status.success());
    assert!(output_text(&changed.stderr).contains("LockedChange"));
    assert_eq!(
        std::fs::read(&lock_path).expect("unchanged lock"),
        first_bytes
    );

    std::fs::remove_file(&lock_path).expect("remove lock for frozen test");
    let frozen = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(&manifest)
        .arg("--frozen")
        .output()
        .expect("frozen missing-lock rejection");
    assert!(!frozen.status.success());
    assert!(output_text(&frozen.stderr).contains("MissingLock"));

    std::fs::remove_dir_all(package).expect("remove temporary Package");
}

#[test]
fn package_run_does_not_ignore_an_invalid_internal_library() {
    let package = temporary_package(
        "invalid-library",
        "namespace Studio.Entry.Library\n\
         public function broken(): Missing\n\
             return missing\n\
         end\n",
        "namespace Studio.Entry.Application\n\
         private function main(arguments: Array<String>): Int\n\
             return 0\n\
         end\n",
    );
    let run = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["run", "--manifestPath"])
        .arg(package.join("bubble.toml"))
        .output()
        .expect("pop run resolves a Package");
    assert!(!run.status.success());
    assert!(
        output_text(&run.stderr).contains("POP1002"),
        "invalid library diagnostic was lost: {}",
        output_text(&run.stderr)
    );
    assert!(!package.join("target/debug/Studio.Entry").exists());
    std::fs::remove_dir_all(package).expect("remove temporary Package");
}

#[test]
fn omitted_main_visibility_defaults_to_internal_in_a_library_bubble() {
    let package = temporary_package(
        "implicit-library-main",
        "namespace Studio.Entry.Library\n\
         function main()\n\
             print(41)\n\
         end\n",
        "namespace Studio.Entry.Application\n\
         function main()\n\
             print(42)\n\
         end\n",
    );
    let run = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["run", "--manifestPath"])
        .arg(package.join("bubble.toml"))
        .output()
        .expect("pop run resolves a Package");
    assert!(
        run.status.success(),
        "stderr:\n{}",
        output_text(&run.stderr)
    );
    assert_eq!(output_text(&run.stdout), "42\n");
    std::fs::remove_dir_all(package).expect("remove temporary Package");
}

#[test]
fn native_class_example_executes_rust_runtime_fields_and_standard_output() {
    let output_path = std::env::temp_dir().join("pop-native-class-example");
    let build = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("build")
        .arg(example("nativeClass.pop"))
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("pop build runs");
    assert!(
        build.status.success(),
        "stderr:\n{}",
        output_text(&build.stderr)
    );
    let executable = Command::new(&output_path)
        .output()
        .expect("native class example runs");
    assert!(executable.status.success());
    assert_eq!(output_text(&executable.stdout), "42\n");
    let _ = std::fs::remove_file(output_path);
}

fn run_check_dump(source: &str, dump: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(fixture(source))
        .args(["--dump", dump])
        .output()
        .expect("pop command runs")
}
