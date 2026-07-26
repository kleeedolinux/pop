use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("cli")
        .join(name)
}

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(arguments)
        .output()
        .expect("pop command runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("Pop Lang presentation is UTF-8")
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' && characters.peek() == Some(&'[') {
            characters.next();
            for code in characters.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

fn normalize_elapsed(value: &str) -> Vec<String> {
    value
        .lines()
        .map(|line| {
            line.split_once(" in ")
                .or_else(|| line.split_once(" after "))
                .map_or(line, |(prefix, _)| prefix)
                .to_owned()
        })
        .collect()
}

#[test]
fn successful_commands_emit_deterministic_plain_progress() {
    let source = fixture("inspectable.pop");
    let first = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(&source)
        .arg("--color")
        .arg("never")
        .output()
        .expect("plain check");
    let second = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("check")
        .arg(&source)
        .arg("--color")
        .arg("never")
        .output()
        .expect("second plain check");

    assert!(first.status.success(), "{}", text(&first.stderr));
    assert!(first.stdout.is_empty());
    let stderr = text(&first.stderr);
    assert!(stderr.contains("Checking"), "{stderr}");
    assert!(stderr.contains("[1/1]"), "{stderr}");
    assert!(stderr.contains("Finished"), "{stderr}");
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");

    assert_eq!(
        normalize_elapsed(&stderr),
        normalize_elapsed(&text(&second.stderr)),
        "plain command facts must not depend on timing"
    );
}

#[test]
fn explicit_interactive_mode_falls_back_without_terminal_streams() {
    let source = fixture("inspectable.pop");
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("--interactive")
        .arg("check")
        .arg(source)
        .args(["--color", "never"])
        .output()
        .expect("non-terminal interactive fallback");

    assert!(output.status.success(), "{}", text(&output.stderr));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("Interactive presentation unavailable"),
        "{stderr}"
    );
    assert!(stderr.contains("Checking"), "{stderr}");
    assert!(stderr.contains("Finished"), "{stderr}");
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
}

#[test]
fn color_policy_is_semantically_neutral_and_explicit_choice_wins() {
    let source = fixture("invalid.pop");
    let never = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("--color")
        .arg("never")
        .arg("check")
        .arg(&source)
        .output()
        .expect("colorless diagnostic");
    let always = Command::new(env!("CARGO_BIN_EXE_pop"))
        .env("NO_COLOR", "1")
        .arg("check")
        .arg(&source)
        .args(["--color", "always"])
        .output()
        .expect("colored diagnostic");

    assert!(!never.status.success());
    assert_eq!(never.status.code(), always.status.code());
    let never_stderr = text(&never.stderr);
    let always_stderr = text(&always.stderr);
    assert!(!never_stderr.contains('\u{1b}'), "{never_stderr:?}");
    assert!(always_stderr.contains('\u{1b}'), "{always_stderr:?}");
    assert!(
        always_stderr.contains("\u{1b}[1;31merror[POP1002]\u{1b}[0m:"),
        "{always_stderr:?}"
    );
    assert!(
        always_stderr.contains("\u{1b}[1;36m  Checking\u{1b}[0m  check"),
        "only the command status label should be cyan: {always_stderr:?}"
    );
    let source_line = always_stderr
        .lines()
        .find(|line| line.contains("return missingValue"))
        .expect("colored source line");
    assert!(source_line.contains("\u{1b}[36m"), "{source_line:?}");
    assert!(
        source_line.find("\u{1b}[0m").expect("neutral reset")
            < source_line
                .find("return missingValue")
                .expect("source text"),
        "source text must be neutral rather than red: {source_line:?}"
    );
    assert!(!source_line.contains("\u{1b}[31m"), "{source_line:?}");
    let caret_line = always_stderr
        .lines()
        .find(|line| line.contains("^^^^^^^^^^^^"))
        .expect("colored caret line");
    assert!(caret_line.contains("\u{1b}[31m"), "{caret_line:?}");
    assert!(
        always_stderr.contains("\u{1b}[1;36m  Progress\u{1b}[0m  Checking [1/1]"),
        "only the progress label should be cyan: {always_stderr:?}"
    );
    assert!(
        always_stderr.contains("\u{1b}[1;31m  Failed\u{1b}[0m  `check`"),
        "only the failure label should be red: {always_stderr:?}"
    );
    assert_eq!(
        normalize_elapsed(&never_stderr),
        normalize_elapsed(&strip_ansi(&always_stderr))
    );
}

#[test]
fn misspelled_return_marks_the_keyword_instead_of_the_following_constructor() {
    let source = fixture("misspelledReturn.pop");
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["--color", "never", "check"])
        .arg(&source)
        .output()
        .expect("misspelled return diagnostic");

    assert!(!output.status.success());
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("error[POP0002]: expected `return`, found `retun`"),
        "{stderr}"
    );
    let source_line = stderr
        .lines()
        .find(|line| line.contains("retun Box"))
        .expect("source line");
    let caret_line = stderr
        .lines()
        .find(|line| line.contains("^^^^^"))
        .expect("caret line");
    assert_eq!(
        source_line.find("retun").expect("misspelled keyword"),
        caret_line.find('^').expect("caret"),
        "{stderr}"
    );
    assert_eq!(caret_line.matches('^').count(), 5, "{stderr}");

    let colored = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["--color", "always", "check"])
        .arg(source)
        .output()
        .expect("colored misspelled return diagnostic");
    let colored_stderr = text(&colored.stderr);
    assert!(
        colored_stderr.contains("\u{1b}[1;36m  help:\u{1b}[0m replace with `return`"),
        "the help label should have its own informational color: {colored_stderr:?}"
    );
}

#[test]
fn no_color_disables_automatic_color() {
    let source = fixture("invalid.pop");
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .env("NO_COLOR", "1")
        .arg("check")
        .arg(source)
        .output()
        .expect("NO_COLOR diagnostic");

    assert!(!output.status.success());
    assert!(!text(&output.stderr).contains('\u{1b}'));
}

#[test]
fn help_documents_complete_presentation_options() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(stdout.contains("--interactive"), "{stdout}");
    assert!(stdout.contains("--color auto|always|never"), "{stdout}");
    assert!(stdout.contains("--messageFormat human|json"), "{stdout}");
    assert!(stdout.contains("--warningWave"), "{stdout}");
    assert!(stdout.contains("--warningsAsErrors"), "{stdout}");
    assert!(stdout.contains("--disabledWarnings"), "{stdout}");
    assert!(stdout.contains("--maximumErrors"), "{stdout}");
}

#[test]
fn json_feedback_is_schema_stable_and_bypasses_terminal_presentation() {
    let source = fixture("invalid.pop");
    let plain_request = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("--messageFormat")
        .arg("json")
        .arg("check")
        .arg(&source)
        .output()
        .expect("JSON diagnostic");
    let interactive_request = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("--interactive")
        .arg("--color")
        .arg("always")
        .arg("check")
        .arg(&source)
        .args(["--messageFormat", "json"])
        .output()
        .expect("JSON bypasses terminal presentation");

    assert!(!plain_request.status.success());
    assert_eq!(
        plain_request.status.code(),
        interactive_request.status.code()
    );
    assert!(
        plain_request.stderr.is_empty(),
        "{:?}",
        plain_request.stderr
    );
    assert!(
        interactive_request.stderr.is_empty(),
        "{:?}",
        interactive_request.stderr
    );
    assert_eq!(
        plain_request.stdout, interactive_request.stdout,
        "JSON facts must not vary with terminal or color requests"
    );
    let stdout = text(&plain_request.stdout);
    assert!(!stdout.contains('\u{1b}'), "{stdout:?}");
    let events = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("one JSON event per line"))
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 4, "{events:#?}");
    assert_eq!(events[0]["schemaVersion"], 1);
    assert_eq!(events[0]["kind"], "commandStarted");
    assert_eq!(events[0]["command"], "check");
    assert_eq!(events[1]["kind"], "diagnostic");
    assert_eq!(events[1]["diagnostic"]["code"], "POP1002");
    assert_eq!(events[1]["diagnostic"]["severity"], "error");
    assert_eq!(events[2]["kind"], "commandProgress");
    assert_eq!(events[2]["completed"], 1);
    assert_eq!(events[2]["total"], 1);
    assert_eq!(events[3]["kind"], "commandFinished");
    assert_eq!(events[3]["outcome"], "failure");
}

#[test]
fn json_diagnostics_include_warning_policy_and_semantic_fix_facts() {
    let source = fixture("export.pop");
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["--messageFormat", "json", "check"])
        .arg(source)
        .output()
        .expect("JSON migration diagnostic");

    assert!(!output.status.success());
    let events = text(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON event"))
        .collect::<Vec<_>>();
    let diagnostic = &events
        .iter()
        .find(|event| event["kind"] == "diagnostic")
        .expect("diagnostic event")["diagnostic"];
    assert_eq!(diagnostic["code"], "POP0004");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["category"], "syntax");
    assert!(diagnostic["arguments"].is_array());
    assert!(diagnostic["primarySpan"].is_object());
    assert!(
        diagnostic["primarySpan"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("export.pop"))
    );
    assert_eq!(diagnostic["primarySpan"]["startPosition"]["line"], 2);
    assert_eq!(diagnostic["primarySpan"]["startPosition"]["column"], 0);
    assert!(diagnostic["labels"].is_array());
    assert!(diagnostic["notes"].is_array());
    assert!(diagnostic["originChain"].is_array());
    assert_eq!(diagnostic["warningWave"], Value::Null);
    assert_eq!(diagnostic["warningGroup"], Value::Null);
    assert_eq!(diagnostic["fixes"][0]["id"], "replaceExportWithPublic");
    assert_eq!(diagnostic["fixes"][0]["applicability"], "safe");
    assert_eq!(
        diagnostic["fixes"][0]["equivalenceKey"],
        "replaceExportWithPublic"
    );
    assert_eq!(
        diagnostic["fixes"][0]["edit"]["edits"][0]["replacement"],
        "public"
    );
}

#[test]
fn enabled_warnings_are_rendered_without_becoming_intrinsic_errors() {
    let source = fixture("warning.pop");
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["--messageFormat", "json", "check"])
        .arg(source)
        .output()
        .expect("JSON warning");

    assert!(output.status.success(), "{}", text(&output.stdout));
    let events = text(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON event"))
        .collect::<Vec<_>>();
    let diagnostics = events
        .iter()
        .filter(|event| event["kind"] == "diagnostic")
        .collect::<Vec<_>>();
    assert!(!diagnostics.is_empty(), "{events:#?}");
    for event in diagnostics {
        assert_eq!(event["diagnostic"]["severity"], "warning");
        assert_eq!(event["diagnostic"]["warningWave"], 1);
        assert_eq!(event["diagnostic"]["warningGroup"], "Documentation");
        assert!(
            event["diagnostic"]["suppressionKey"]
                .as_str()
                .is_some_and(|key| key.starts_with("POP"))
        );
    }
    assert_eq!(events.last().expect("finished event")["outcome"], "success");
}

#[test]
fn warning_policy_controls_wave_promotion_and_disabling_without_mutating_severity() {
    let warning = fixture("warning.pop");
    let wave_zero = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["--messageFormat", "json", "--warningWave", "0", "check"])
        .arg(&warning)
        .output()
        .expect("wave zero");
    assert!(wave_zero.status.success(), "{}", text(&wave_zero.stdout));
    assert!(
        !text(&wave_zero.stdout).contains("\"kind\":\"diagnostic\""),
        "{}",
        text(&wave_zero.stdout)
    );

    let disabled = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args([
            "--messageFormat",
            "json",
            "--disabledWarnings",
            "POP6400",
            "check",
        ])
        .arg(&warning)
        .output()
        .expect("disabled warning");
    assert!(disabled.status.success(), "{}", text(&disabled.stdout));
    assert!(!text(&disabled.stdout).contains("\"kind\":\"diagnostic\""));

    let promoted = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args([
            "--messageFormat",
            "json",
            "--warningsAsErrors",
            "Documentation",
            "check",
        ])
        .arg(warning)
        .output()
        .expect("promoted warning");
    assert!(!promoted.status.success());
    let diagnostic = text(&promoted.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event"))
        .find(|event| event["kind"] == "diagnostic")
        .expect("warning event");
    assert_eq!(diagnostic["diagnostic"]["severity"], "warning");
    assert_eq!(diagnostic["diagnostic"]["policy"]["promoted"], true);
    assert_eq!(diagnostic["diagnostic"]["policy"]["blocksArtifact"], true);

    let source_error = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args([
            "--messageFormat",
            "json",
            "--disabledWarnings",
            "*",
            "check",
        ])
        .arg(fixture("invalid.pop"))
        .output()
        .expect("error cannot be disabled");
    assert!(!source_error.status.success());
    assert!(text(&source_error.stdout).contains("\"code\":\"POP1002\""));
}

#[test]
fn maximum_errors_bounds_output_and_reports_exact_omitted_count() {
    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["--messageFormat", "json", "--maximumErrors", "1", "check"])
        .arg(fixture("manyErrors.pop"))
        .output()
        .expect("bounded errors");

    assert!(!output.status.success());
    let events = text(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event"))
        .collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["kind"] == "diagnostic")
            .count(),
        1
    );
    let limit = events
        .iter()
        .find(|event| event["kind"] == "diagnosticLimitReached")
        .expect("limit event");
    assert_eq!(limit["maximumErrors"], 1);
    assert_eq!(limit["omittedErrors"], 1);
}

#[test]
fn fix_applies_verified_safe_edits_atomically_and_is_idempotent() {
    let root = std::env::temp_dir().join(format!("pop-driver-fix-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temporary fix directory");
    let source = root.join("migration.pop");
    fs::copy(fixture("export.pop"), &source).expect("copy migration source");

    let first = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("fix")
        .arg(&source)
        .args(["--color", "never"])
        .output()
        .expect("safe fix");
    assert!(first.status.success(), "{}", text(&first.stderr));
    let fixed = fs::read_to_string(&source).expect("fixed source");
    assert!(fixed.contains("public function answer"), "{fixed}");
    assert!(!fixed.contains("export function"), "{fixed}");
    assert!(text(&first.stderr).contains("Applied 1 safe fix"));

    let second = Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["--messageFormat", "json", "fix"])
        .arg(&source)
        .output()
        .expect("idempotent fix");
    assert!(second.status.success(), "{}", text(&second.stdout));
    assert_eq!(fs::read_to_string(&source).expect("stable source"), fixed);
    let summary = text(&second.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event"))
        .find(|event| event["kind"] == "fixSummary")
        .expect("fix summary");
    assert_eq!(summary["appliedFixes"], 0);
    assert_eq!(summary["changedDocuments"], 0);

    fs::remove_dir_all(root).expect("remove temporary fix directory");
}

#[test]
fn fix_does_not_mutate_source_when_no_proven_safe_correction_exists() {
    let root = std::env::temp_dir().join(format!("pop-driver-fix-rejected-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temporary fix directory");
    let source = root.join("invalid.pop");
    fs::copy(fixture("invalid.pop"), &source).expect("copy invalid source");
    let before = fs::read(&source).expect("original bytes");

    let output = Command::new(env!("CARGO_BIN_EXE_pop"))
        .arg("fix")
        .arg(&source)
        .args(["--color", "never"])
        .output()
        .expect("fix without safe correction");

    assert!(!output.status.success());
    assert_eq!(fs::read(&source).expect("unchanged bytes"), before);
    assert!(text(&output.stderr).contains("POP1002"));
    fs::remove_dir_all(root).expect("remove temporary fix directory");
}
