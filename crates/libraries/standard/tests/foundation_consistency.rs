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
    assert!(
        !section.contains("- [ ]"),
        "section 2 must not claim unresolved work after its accepted completion boundary"
    );
}
