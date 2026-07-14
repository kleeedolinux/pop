use std::fs;
use std::path::{Path, PathBuf};

use pop_driver::{FrontEndBubbleInput, FrontEndModule, FrontEndResult, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_projects::{BubbleKind, discover_conventional_bubbles, parse_package_manifest};
use pop_source::SourceFile;

const INTERNAL_BUBBLE: BubbleId = BubbleId::from_raw(1);
const STANDARD_BUBBLE: BubbleId = BubbleId::from_raw(2);

#[derive(Clone, Copy)]
struct Contribution<'source> {
    path: &'source str,
    source: &'source str,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("test-runner crate is under the repository root")
        .to_owned()
}

fn collect_pop_paths(directory: &Path, root: &Path, paths: &mut Vec<String>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read source directory {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.expect("read source entry");
        let path = entry.path();
        if path.is_dir() {
            collect_pop_paths(&path, root, paths);
        } else if path.extension().is_some_and(|extension| extension == "pop") {
            let relative = path
                .strip_prefix(root)
                .expect("source path is below its foundation root");
            paths.push(
                relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
}

fn analyze_foundation(
    relative_root: &str,
    expected_name: &str,
    expected_dependency_aliases: &[&str],
    bubble: BubbleId,
    dependencies: Vec<BubbleId>,
    contribution: Contribution<'_>,
) -> FrontEndResult {
    let source_root = repository_root().join(relative_root);
    let manifest_text = fs::read_to_string(source_root.join("bubble.toml"))
        .unwrap_or_else(|error| panic!("read {relative_root}/bubble.toml: {error}"));
    let manifest =
        parse_package_manifest(&manifest_text).expect("valid foundation source manifest");
    assert_eq!(manifest.name(), expected_name);
    assert_eq!(
        manifest
            .dependencies()
            .iter()
            .map(pop_projects::DependencyRequirement::alias)
            .collect::<Vec<_>>(),
        expected_dependency_aliases
    );
    let mut paths = Vec::new();
    collect_pop_paths(&source_root.join("src"), &source_root, &mut paths);
    paths.push(contribution.path.to_owned());
    paths.sort();

    let discovered = discover_conventional_bubbles(&manifest, &paths)
        .expect("discover foundation source Bubble");
    let [library] = discovered.as_slice() else {
        panic!("foundation source root must discover exactly one Bubble");
    };
    assert_eq!(library.kind(), BubbleKind::Library);
    assert_eq!(library.name(), manifest.name());

    let modules = library
        .modules()
        .iter()
        .enumerate()
        .map(|(index, relative)| {
            let text = if relative == contribution.path {
                contribution.source.to_owned()
            } else {
                fs::read_to_string(source_root.join(relative))
                    .unwrap_or_else(|error| panic!("read {relative_root}/{relative}: {error}"))
            };
            let raw = u32::try_from(index).expect("foundation Module count fits typed IDs");
            let source = SourceFile::new(FileId::from_raw(raw), relative.clone(), text)
                .expect("foundation source fits compiler limits");
            FrontEndModule::new(ModuleId::from_raw(raw), source)
        })
        .collect();

    analyze_bubble(FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(bubble.raw()),
        dependencies,
        modules,
    ))
}

#[test]
fn conventionally_discovered_foundation_contributions_reach_verified_mir() {
    let internal = analyze_foundation(
        "crates/libraries/internal/pop",
        "Pop.Internal",
        &[],
        INTERNAL_BUBBLE,
        Vec::new(),
        Contribution {
            path: "src/contributionProbe.pop",
            source: "namespace Pop.Internal.Contribution\n\
                     function trustedIdentity(value: Int): Int\n\
                         return value\n\
                     end\n",
        },
    );
    assert!(
        internal.diagnostics().is_empty(),
        "{}",
        internal.diagnostic_snapshot()
    );
    let internal_hir = internal.hir().expect("verified Pop.Internal HIR");
    assert!(
        internal_hir
            .functions()
            .iter()
            .any(|function| function.name() == "trustedIdentity")
    );
    assert!(internal_hir.public_symbols().is_empty());
    pop_mir::lower_hir_bubble(internal_hir, internal.types())
        .expect("verified Pop.Internal canonical MIR");

    let standard = analyze_foundation(
        "crates/libraries/standard/pop",
        "Pop.Standard",
        &["PopInternal"],
        STANDARD_BUBBLE,
        vec![INTERNAL_BUBBLE],
        Contribution {
            path: "src/contributionProbe.pop",
            source: "namespace Pop.Math\n\
                     using Pop.Sequence\n\
                     public function contributorIdentity(value: Int): Int\n\
                         return value\n\
                     end\n\
                     public function sequenceProbe(): List<Int>\n\
                         local values: {Int} = {1, 2, 3}\n\
                         local mapped = map(values, function(value: Int): Int\n\
                             return value * 2\n\
                         end)\n\
                         local filtered = filter(mapped, function(value: Int): Boolean\n\
                             return value > 2\n\
                         end)\n\
                         return collect(filtered)\n\
                     end\n",
        },
    );
    assert!(
        standard.diagnostics().is_empty(),
        "{}",
        standard.diagnostic_snapshot()
    );
    let standard_hir = standard.hir().expect("verified Pop.Standard HIR");
    assert_eq!(standard_hir.dependencies(), &[INTERNAL_BUBBLE]);
    let contribution = standard_hir
        .functions()
        .iter()
        .find(|function| function.name() == "contributorIdentity")
        .expect("contribution function is discovered without a central registry");
    assert!(
        standard_hir
            .public_symbols()
            .contains(&contribution.symbol())
    );
    for algorithm in ["map", "filter", "fold", "collect"] {
        let function = standard_hir
            .functions()
            .iter()
            .find(|function| function.name() == algorithm)
            .unwrap_or_else(|| panic!("ordinary Pop Sequence.{algorithm} implementation"));
        assert!(standard_hir.public_symbols().contains(&function.symbol()));
        assert!(!function.type_parameters().is_empty());
    }
    assert!(
        standard_hir
            .functions()
            .iter()
            .any(|function| function.name() == "sequenceProbe")
    );
    pop_mir::lower_hir_bubble(standard_hir, standard.types())
        .expect("verified Pop.Standard canonical MIR");

    let metadata = standard
        .reference_metadata()
        .expect("portable Pop.Standard metadata")
        .clone();
    let consumer_source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Application\n\
         using Pop.Sequence\n\
         public function run(): Int\n\
             local values: {Int} = {1, 2, 3}\n\
             local total = fold(values, 0, function(state: Int, value: Int): Int\n\
                 return state + value\n\
             end)\n\
             local mapped = map(values, function(value: Int): Int\n\
                 return value * 2\n\
             end)\n\
             local filtered = filter(mapped, function(value: Int): Boolean\n\
                 return value > 2\n\
             end)\n\
             local collected = collect(filtered)\n\
             local labels = map(values, function(value: Int): String\n\
                 return \"item\"\n\
             end)\n\
             local collectedLabels = collect(labels)\n\
             return total + List.length(collected) + List.length(collectedLabels)\n\
         end\n",
    )
    .expect("consumer source");
    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(7),
            NamespaceId::from_raw(7),
            vec![STANDARD_BUBBLE],
            vec![FrontEndModule::new(ModuleId::from_raw(0), consumer_source)],
        )
        .with_reference_metadata(vec![metadata]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let mir = pop_mir::lower_hir_bubble(
        consumer.hir().expect("Sequence consumer HIR"),
        consumer.types(),
    )
    .expect("portable Sequence algorithms specialize into consumer MIR");
    assert!(!mir.dump().contains("callReference b2:"));
}

#[test]
fn foundation_source_build_rejects_an_invalid_contribution() {
    let result = analyze_foundation(
        "crates/libraries/standard/pop",
        "Pop.Standard",
        &["PopInternal"],
        STANDARD_BUBBLE,
        vec![INTERNAL_BUBBLE],
        Contribution {
            path: "src/invalidContribution.pop",
            source: "namespace Pop.Math\n\
                     public function broken(value: Int): String\n\
                         return value\n\
                     end\n",
        },
    );

    assert!(!result.diagnostics().is_empty());
    assert!(result.hir().is_none());
}
