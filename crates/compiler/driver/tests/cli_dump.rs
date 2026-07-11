use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("cli")
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
fn check_accepts_repeatable_dump_options_in_command_line_order() {
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(fixture("inspectable.pop"))
        .args(["--dump", "hir", "--dump", "mir"])
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
    assert!(hir < mir, "requested dump order must be preserved");
    assert_eq!(stdout.matches("hir bubble").count(), 1);
    assert_eq!(stdout.matches("mir bubble").count(), 1);
}

#[test]
fn invalid_source_emits_a_structured_diagnostic_and_no_dump() {
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(fixture("invalid.pop"))
        .args(["--dump", "hir", "--dump", "mir"])
        .output()
        .expect("pop command runs");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "invalid HIR/MIR must not be dumped"
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
    assert!(output_text(&output.stderr).contains("hir|mir"));
}

#[test]
fn missing_check_arguments_are_a_usage_error() {
    let output = run_pop(&["check"], None);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("pop check"));
}

fn run_check_dump(source: &str, dump: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(fixture(source))
        .args(["--dump", dump])
        .output()
        .expect("pop command runs")
}
