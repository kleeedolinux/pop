use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temporary_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pop-scaffold-{name}-{}", std::process::id()))
}

fn run(arguments: &[&str], current: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pop"));
    command.args(arguments);
    if let Some(current) = current {
        command.current_dir(current);
    }
    command.output().expect("pop command runs")
}

#[test]
fn new_creates_an_exact_buildable_binary_package() {
    let root = temporary_path("binary");
    let _ = std::fs::remove_dir_all(&root);
    let output = run(
        &["new", root.to_str().unwrap(), "--name", "Studio.Hello"],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("bubble.toml")).unwrap(),
        "[package]\nname = \"Studio.Hello\"\nversion = \"0.1.0\"\nedition = \"2026\"\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/main.pop")).unwrap(),
        "namespace Studio.Hello\n\nfunction main()\nend\n"
    );
    let check = run(
        &[
            "check",
            "--manifestPath",
            root.join("bubble.toml").to_str().unwrap(),
        ],
        None,
    );
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn initialize_defaults_to_the_current_directory_and_creates_a_library() {
    let root = temporary_path("library");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let output = run(
        &["initialize", "--name", "Studio.Core", "--library"],
        Some(&root),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.pop")).unwrap(),
        "namespace Studio.Core\n"
    );
    assert!(!root.join("src/main.pop").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn scaffolding_never_rewrites_names_or_overwrites_user_work() {
    let invalid = temporary_path("invalid-name");
    let _ = std::fs::remove_dir_all(&invalid);
    let invalid_output = run(&["new", invalid.to_str().unwrap()], None);
    assert_eq!(invalid_output.status.code(), Some(2));
    assert!(!invalid.exists());

    let existing = temporary_path("existing");
    let _ = std::fs::remove_dir_all(&existing);
    std::fs::create_dir_all(existing.join("src")).unwrap();
    std::fs::write(existing.join("src/main.pop"), "user work\n").unwrap();
    let output = run(
        &[
            "initialize",
            existing.to_str().unwrap(),
            "--name",
            "Studio.Existing",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        std::fs::read_to_string(existing.join("src/main.pop")).unwrap(),
        "user work\n"
    );
    assert!(!existing.join("bubble.toml").exists());
    std::fs::remove_dir_all(existing).unwrap();
}

#[test]
fn initialize_preserves_an_existing_source_directory() {
    let root = temporary_path("existing-source");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/readme.txt"), "keep\n").unwrap();
    let output = run(
        &[
            "initialize",
            root.to_str().unwrap(),
            "--name",
            "Studio.ExistingSource",
            "--library",
        ],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/readme.txt")).unwrap(),
        "keep\n"
    );
    assert!(root.join("src/lib.pop").is_file());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn conflicting_scaffold_kinds_are_rejected_before_writing() {
    let root = temporary_path("kind-conflict");
    let _ = std::fs::remove_dir_all(&root);
    let output = run(
        &[
            "new",
            root.to_str().unwrap(),
            "--name",
            "Studio.Conflict",
            "--library",
            "--binary",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(!root.exists());
}
