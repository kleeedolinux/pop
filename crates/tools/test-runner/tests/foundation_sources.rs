#![allow(clippy::too_many_lines)]

use std::fs;
use std::path::{Path, PathBuf};

use pop_documentation::XmlNode;
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

fn collect_documentation_contracts(
    nodes: &[XmlNode],
    names: &mut Vec<String>,
    examples: &mut Vec<String>,
) {
    for node in nodes {
        let XmlNode::Element {
            name,
            attributes,
            children,
        } = node
        else {
            continue;
        };
        names.push(name.clone());
        if name == "code"
            && attributes
                .iter()
                .any(|attribute| attribute.name() == "language" && attribute.value() == "pop")
            && attributes
                .iter()
                .any(|attribute| attribute.name() == "test" && attribute.value() == "true")
        {
            examples.push(
                children
                    .iter()
                    .filter_map(|child| match child {
                        XmlNode::Text(text) => Some(text.as_str()),
                        XmlNode::Element { .. } => None,
                    })
                    .collect(),
            );
        }
        collect_documentation_contracts(children, names, examples);
    }
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

fn verify_internal_foundation_contribution() {
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
}

fn analyze_standard_foundation_contribution() -> FrontEndResult {
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
                             if value == 1 then\n\
                                 return 2\n\
                             end\n\
                             if value == 2 then\n\
                                 return 4\n\
                             end\n\
                             if value == 3 then\n\
                                 return 6\n\
                             end\n\
                             return 0\n\
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
    for algorithm in [
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
        let function = standard_hir
            .functions()
            .iter()
            .find(|function| function.name() == algorithm)
            .unwrap_or_else(|| panic!("ordinary Pop Sequence.{algorithm} implementation"));
        assert!(standard_hir.public_symbols().contains(&function.symbol()));
        assert!(!function.type_parameters().is_empty());
    }
    for function_name in ["min", "max", "abs", "gcd", "sign", "lcm", "coprime"] {
        let function = standard_hir
            .functions()
            .iter()
            .find(|function| function.name() == function_name)
            .unwrap_or_else(|| panic!("ordinary Pop Math.{function_name} implementation"));
        assert!(standard_hir.public_symbols().contains(&function.symbol()));
        assert!(function.type_parameters().is_empty());
    }
    assert!(
        standard_hir
            .functions()
            .iter()
            .any(|function| function.name() == "sequenceProbe")
    );
    pop_mir::lower_hir_bubble(standard_hir, standard.types())
        .expect("verified Pop.Standard canonical MIR");

    let documentation = standard.checked_documentation();
    assert_eq!(
        documentation.len(),
        42,
        "every portable public API is documented"
    );
    let mut examples = Vec::new();
    for member in documentation {
        let function = standard_hir
            .functions()
            .iter()
            .find(|function| function.symbol() == member.identity().symbol())
            .expect("documentation belongs to a public function");
        let mut names = Vec::new();
        collect_documentation_contracts(member.fragment().children(), &mut names, &mut examples);
        for required in ["summary", "allocation", "complexity"] {
            assert!(
                names.iter().any(|name| name == required),
                "public function {} documentation lacks <{required}>",
                function.name()
            );
        }
        if ["map", "filter", "fold", "collect"].contains(&function.name()) {
            for required in [
                "typeparam",
                "param",
                "returns",
                "remarks",
                "example",
                "code",
            ] {
                assert!(
                    names.iter().any(|name| name == required),
                    "Sequence.{} documentation lacks <{required}>",
                    function.name()
                );
            }
        }
    }
    assert_eq!(examples.len(), 4, "baseline examples remain compiled");

    let example_source = SourceFile::new(
        FileId::from_raw(0),
        "examples/sequence.pop",
        format!("namespace StandardExamples\n{}\n", examples.join("\n")),
    )
    .expect("compiled documentation example source");
    let compiled_examples = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(8),
            NamespaceId::from_raw(8),
            vec![STANDARD_BUBBLE],
            vec![FrontEndModule::new(ModuleId::from_raw(0), example_source)],
        )
        .with_reference_metadata(vec![
            standard
                .reference_metadata()
                .expect("portable Pop.Standard metadata")
                .clone(),
        ]),
    );
    assert!(
        compiled_examples.diagnostics().is_empty(),
        "{}",
        compiled_examples.diagnostic_snapshot()
    );
    pop_mir::lower_hir_bubble(
        compiled_examples.hir().expect("documentation example HIR"),
        compiled_examples.types(),
    )
    .expect("documentation examples lower to verified MIR");

    standard
}

fn verify_sequence_consumer(standard: &FrontEndResult) {
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
                 if state == 0 and value == 1 then\n\
                     return 1\n\
                 end\n\
                 if state == 1 and value == 2 then\n\
                     return 12\n\
                 end\n\
                 if state == 12 and value == 3 then\n\
                     return 123\n\
                 end\n\
                 return -1\n\
             end)\n\
             local mapped = map(values, function(value: Int): Int\n\
                 if value == 1 then\n\
                     return 2\n\
                 end\n\
                 if value == 2 then\n\
                     return 4\n\
                 end\n\
                 if value == 3 then\n\
                     return 6\n\
                 end\n\
                 return 0\n\
             end)\n\
             local filtered = filter(mapped, function(value: Int): Boolean\n\
                 return value > 2\n\
             end)\n\
             local collected = collect(filtered)\n\
             local labels = map(values, function(value: Int): String\n\
                 return \"item\"\n\
             end)\n\
             local collectedLabels = collect(labels)\n\
             local hasLarge = any(values, function(value: Int): Boolean\n\
                 return value > 2\n\
             end)\n\
             local allPositive = all(values, function(value: Int): Boolean\n\
                 return value > 0\n\
             end)\n\
             local noHuge = none(values, function(value: Int): Boolean\n\
                 return value > 3\n\
             end)\n\
             each(values, function(value: Int)\n\
             end)\n\
             local selected = countWhere(values, function(value: Int): Boolean\n\
                 return value > 1\n\
             end)\n\
             local requested = elementAtOr(values, 2, 0)\n\
             local lastMatch = findLastOr(values, function(value: Int): Boolean\n\
                 return value > 1\n\
             end, 0)\n\
             local lastPosition = indexLastOr(values, function(value: Int): Boolean\n\
                 return value > 1\n\
             end, 0)\n\
             local reduced = reduceOr(values, function(state: Int, value: Int): Int\n\
                 if state == 1 and value == 2 then\n\
                     return 12\n\
                 end\n\
                 if state == 12 and value == 3 then\n\
                     return 123\n\
                 end\n\
                 return -1\n\
             end, 0)\n\
             local window = collect(take(drop(values, 1), 1))\n\
             local joined = collect(concat(window, values))\n\
             local numeric = sum(values) + product(values) + minOr(values, 0) + maxOr(values, 0)\n\
             if not hasLarge or not allPositive or not noHuge then\n\
                 return -1\n\
             end\n\
             return total + List.length(collected) + List.length(collectedLabels) + List.length(joined) + count(values) + selected + requested + lastMatch + lastPosition + reduced + firstOr(values, 0) + lastOr(values, 0) + numeric\n\
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
    let hir = consumer.hir().expect("Sequence consumer HIR");
    let dump = hir.dump(consumer.types());
    let mir = pop_mir::lower_hir_bubble(hir, consumer.types()).unwrap_or_else(|errors| {
        panic!("portable Sequence algorithms specialize into consumer MIR: {errors:?}\n{dump}")
    });
    assert!(!mir.dump().contains("callReference b2:"));
}

#[test]
fn conventionally_discovered_foundation_contributions_reach_verified_mir() {
    verify_internal_foundation_contribution();
    let standard = analyze_standard_foundation_contribution();
    verify_sequence_consumer(&standard);
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

#[test]
fn foundation_sequence_rejects_invalid_callbacks() {
    for source in [
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Boolean\n\
             return any(values, function(value: Int): Int\n\
                 return value\n\
             end)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return countWhere(values, function(value: Int): Int\n\
                 return value\n\
             end)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int})\n\
             each(values, function(value: Int): Int\n\
                 return value\n\
             end)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return count(take(values, 1.0))\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return count(takeWhile(values, function(value: Int): Int\n\
                 return value\n\
             end))\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {String}): Int\n\
             return sum(values)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return findOr(values, function(value: Int): Int\n\
                 return value\n\
             end, 0)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return sumBy(values, function(value: Int): Boolean\n\
                 return value > 0\n\
             end)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Iterator<Int>\n\
             return scan(values, 0, function(state: String, value: Int): Int\n\
                 return value\n\
             end)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return elementAtOr(values, \"second\", 0)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return findLastOr(values, function(value: Int): Int\n\
                 return value\n\
             end, 0)\n\
         end\n",
        "namespace Pop.Sequence.Contribution\n\
         using Pop.Sequence\n\
         public function broken(values: {Int}): Int\n\
             return reduceOr(values, function(left: Int, right: Int): String\n\
                 return \"wrong\"\n\
             end, 0)\n\
         end\n",
    ] {
        let result = analyze_foundation(
            "crates/libraries/standard/pop",
            "Pop.Standard",
            &["PopInternal"],
            STANDARD_BUBBLE,
            vec![INTERNAL_BUBBLE],
            Contribution {
                path: "src/invalidSequenceContribution.pop",
                source,
            },
        );

        assert!(!result.diagnostics().is_empty(), "{source}");
        assert!(result.hir().is_none(), "{source}");
    }
}

#[test]
fn foundation_math_rejects_non_integer_calls() {
    for expression in ["min(1.0, 2.0)", "sign(1.0)", "lcm(1, 2.0)"] {
        let source = format!(
            "namespace Pop.Math.Contribution\n\
             using Pop.Math\n\
             public function broken(): Int\n\
                 return {expression}\n\
             end\n"
        );
        let result = analyze_foundation(
            "crates/libraries/standard/pop",
            "Pop.Standard",
            &["PopInternal"],
            STANDARD_BUBBLE,
            vec![INTERNAL_BUBBLE],
            Contribution {
                path: "src/invalidMathContribution.pop",
                source: &source,
            },
        );

        assert!(!result.diagnostics().is_empty(), "{expression}");
        assert!(result.hir().is_none(), "{expression}");
    }
}
