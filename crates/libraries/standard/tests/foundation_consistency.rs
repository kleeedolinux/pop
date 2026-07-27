use std::fs;
use std::path::{Path, PathBuf};

use pop_standard::standard_api_baseline;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("standard crate is below the repository root")
        .to_owned()
}

fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

#[test]
fn frozen_foundation_baseline_and_delivery_status_stay_consistent() {
    let root = repository_root();
    let baseline = standard_api_baseline().expect("valid standard API baseline");

    for entry in baseline.entries() {
        assert!(
            root.join(entry.documentation()).is_file(),
            "{} names missing authority {}",
            entry.identity(),
            entry.documentation()
        );
    }

    let catalog = read(
        &root,
        "architecture/22.1-core-and-portable-library-catalog.md",
    );
    assert!(
        catalog.contains("optional `T?` values"),
        "the active catalog must use the ADR 0058 optional-value contract"
    );
    assert!(
        !catalog.contains("records, `Option`, `Result`"),
        "the active catalog must not revive a nominal Option wrapper"
    );

    let roadmap = read(&root, "ROADMAP.md");
    let section = roadmap
        .split_once("### 2. Finish the standard foundation")
        .expect("standard-foundation roadmap section")
        .1
        .split_once("### 3. Make the runtime release-ready")
        .expect("runtime roadmap section follows the standard foundation")
        .0;
    let (frozen_foundation, modern_essential) = section
        .split_once("#### Complete the modern essential libraries")
        .expect("ADR 0110 modern essential-library boundary");
    assert!(
        !frozen_foundation.contains("- [ ]"),
        "the frozen bootstrap foundation must remain complete"
    );
    let open_groups = modern_essential
        .lines()
        .filter(|line| line.starts_with("- [ ]"))
        .collect::<Vec<_>>();
    assert_eq!(
        open_groups.len(),
        6,
        "ADR 0110 keeps all six modern delivery groups open until their exact profiles complete"
    );
    for required in [
        "Complete core values:",
        "Complete formats and deterministic data boundaries:",
        "Complete portable and target-qualified host foundations:",
        "Complete structured concurrency and secure network foundations:",
        "Complete standard `Telemetry` contracts",
        "Build the independent `Pop.Http` Package",
    ] {
        assert!(
            open_groups.iter().any(|line| line.contains(required)),
            "missing open modern-foundation delivery group `{required}`"
        );
    }
}
