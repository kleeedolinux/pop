use std::fmt::Write as _;

use pop_standard::{
    ApiBaselineError, ApiKind, ApiStatus, parse_standard_api_baseline, standard_api_baseline,
};
use pop_types::embedded_bootstrap_schema;

#[test]
fn frozen_standard_api_baseline_has_exact_prelude_and_prototype_boundaries() {
    let baseline = standard_api_baseline().expect("valid embedded API baseline");
    assert_eq!(baseline.schema_version(), 1);
    assert_eq!(baseline.entries().len(), 70);

    let prelude_names = baseline
        .entries()
        .iter()
        .filter(|entry| entry.prelude())
        .map(|entry| (entry.kind(), entry.name()))
        .collect::<Vec<_>>();
    assert!(prelude_names.contains(&(ApiKind::Namespace, "Sequence")));
    assert!(!prelude_names.contains(&(ApiKind::Namespace, "Math")));
    assert!(!prelude_names.iter().any(|(_, name)| *name == "Option"));
    assert!(!prelude_names.iter().any(|(_, name)| *name == "Actor"));
    assert!(!prelude_names.iter().any(|(_, name)| *name == "Cluster"));

    let prototypes = baseline
        .entries()
        .iter()
        .filter(|entry| entry.status() == ApiStatus::Prototype)
        .map(|entry| entry.identity())
        .collect::<Vec<_>>();
    assert_eq!(prototypes.len(), 33);
    assert_eq!(
        &prototypes[..4],
        ["namespace:0", "namespace:1", "function:0", "function:1"]
    );
    assert_eq!(prototypes.last(), Some(&"api:28"));

    let portable_names = baseline
        .entries()
        .iter()
        .filter(|entry| entry.kind() == ApiKind::Api)
        .map(|entry| (entry.namespace(), entry.name()))
        .collect::<Vec<_>>();
    assert_eq!(
        portable_names,
        [
            ("Pop.Sequence", "map"),
            ("Pop.Sequence", "filter"),
            ("Pop.Sequence", "fold"),
            ("Pop.Sequence", "collect"),
            ("Pop.Sequence", "any"),
            ("Pop.Sequence", "all"),
            ("Pop.Sequence", "count"),
            ("Pop.Math", "min"),
            ("Pop.Math", "max"),
            ("Pop.Math", "abs"),
            ("Pop.Math", "gcd"),
            ("Pop.Sequence", "isEmpty"),
            ("Pop.Sequence", "firstOr"),
            ("Pop.Sequence", "lastOr"),
            ("Pop.Sequence", "each"),
            ("Pop.Sequence", "none"),
            ("Pop.Sequence", "countWhere"),
            ("Pop.Math", "sign"),
            ("Pop.Math", "lcm"),
            ("Pop.Math", "coprime"),
            ("Pop.Sequence", "take"),
            ("Pop.Sequence", "drop"),
            ("Pop.Sequence", "takeWhile"),
            ("Pop.Sequence", "dropWhile"),
            ("Pop.Sequence", "concat"),
            ("Pop.Sequence", "sum"),
            ("Pop.Sequence", "product"),
            ("Pop.Sequence", "minOr"),
            ("Pop.Sequence", "maxOr"),
        ]
    );
}

#[test]
fn standard_api_baseline_agrees_with_trusted_bootstrap_identities() {
    let baseline = standard_api_baseline().expect("valid embedded API baseline");
    let bootstrap = embedded_bootstrap_schema().expect("valid bootstrap metadata");

    for entry in baseline.entries() {
        let (_, raw_id) = entry.identity().split_once(':').expect("baseline identity");
        match entry.kind() {
            ApiKind::Primitive => assert!(
                bootstrap
                    .primitives()
                    .iter()
                    .any(|primitive| primitive.source_name() == entry.name())
            ),
            ApiKind::Type => {
                let metadata = bootstrap
                    .type_by_source_name(entry.name())
                    .unwrap_or_else(|| panic!("missing bootstrap type {}", entry.name()));
                assert_eq!(metadata.id().raw().to_string(), raw_id);
                assert_eq!(metadata.owner_bubble(), entry.owner_bubble());
                assert_eq!(metadata.is_in_prelude(), entry.prelude());
            }
            ApiKind::Attribute => {
                let metadata = bootstrap
                    .compiler_attributes()
                    .iter()
                    .find(|attribute| attribute.source_name() == entry.name())
                    .unwrap_or_else(|| panic!("missing bootstrap attribute {}", entry.name()));
                assert_eq!(metadata.id().raw().to_string(), raw_id);
                assert_eq!(metadata.owner_bubble(), entry.owner_bubble());
                assert_eq!(metadata.is_in_prelude(), entry.prelude());
            }
            ApiKind::Function if entry.namespace() == "Pop" => {
                let metadata = bootstrap
                    .standard_functions()
                    .iter()
                    .find(|function| function.id().raw().to_string() == raw_id)
                    .unwrap_or_else(|| panic!("missing bootstrap function {}", entry.identity()));
                assert_eq!(metadata.source_name(), entry.name());
                assert_eq!(metadata.owner_bubble(), entry.owner_bubble());
                assert_eq!(metadata.is_in_prelude(), entry.prelude());
            }
            ApiKind::Namespace | ApiKind::Api | ApiKind::Function => {}
        }
    }
}

#[test]
fn standard_api_baseline_rejects_noncanonical_or_unsupported_metadata() {
    let header = "schemaVersion\t1\nidentity\tkind\townerBubble\tnamespace\tname\tsignature\ttier\tstatus\tprelude\tdocumentation\n";
    let valid = "primitive:0\tPrimitive\tPop.Internal\tPop\tBoolean\tBoolean\tprelude\timplemented\ttrue\tarchitecture/02-language-model.md\n";

    assert!(parse_standard_api_baseline(&(header.to_owned() + valid)).is_ok());
    for invalid in [
        header.to_owned() + valid + valid,
        header.to_owned()
            + "primitive:0\tUnknown\tPop.Internal\tPop\tBoolean\tBoolean\tprelude\timplemented\ttrue\tarchitecture/02-language-model.md\n",
        header.to_owned()
            + "primitive:0\tPrimitive\tPop.Internal\tPop\tBoolean\tBoolean\tprelude\tplanned\ttrue\tarchitecture/02-language-model.md\n",
        header.to_owned()
            + "primitive:1\tPrimitive\tPop.Internal\tPop\tBoolean\tBoolean\tprelude\timplemented\ttrue\tarchitecture/02-language-model.md\n"
            + valid,
    ] {
        assert_eq!(
            parse_standard_api_baseline(&invalid),
            Err(ApiBaselineError::InvalidEntry)
        );
    }
}

#[test]
fn standard_api_baseline_rejects_noncanonical_identity_namespace_and_tier_fields() {
    let header = "schemaVersion\t1\nidentity\tkind\townerBubble\tnamespace\tname\tsignature\ttier\tstatus\tprelude\tdocumentation\n";
    for invalid_entry in [
        "primitive:00\tPrimitive\tPop.Internal\tPop\tBoolean\tBoolean\tprelude\timplemented\ttrue\tarchitecture/02-language-model.md\n",
        "primitive:0\tPrimitive\tPop.Internal\tPopcorn\tBoolean\tBoolean\tprelude\timplemented\ttrue\tarchitecture/02-language-model.md\n",
        "primitive:0\tPrimitive\tPop.Internal\tPop\tBoolean\tBoolean\tprelude\timplemented\tfalse\tarchitecture/02-language-model.md\n",
        "primitive:0\tPrimitive\tPop.Internal\tPop\tBoolean\tBoolean\tprelude\timplemented\ttrue\tarchitecture/../ROADMAP.md\n",
    ] {
        assert_eq!(
            parse_standard_api_baseline(&(header.to_owned() + invalid_entry)),
            Err(ApiBaselineError::InvalidEntry)
        );
    }
}

#[test]
fn standard_api_baseline_loading_is_bounded() {
    let header = "schemaVersion\t1\nidentity\tkind\townerBubble\tnamespace\tname\tsignature\ttier\tstatus\tprelude\tdocumentation\n";
    let oversized_entry = format!(
        "primitive:0\tPrimitive\tPop.Internal\tPop\t{}\tBoolean\tprelude\timplemented\ttrue\tarchitecture/02-language-model.md\n",
        "A".repeat(5_000)
    );
    assert_eq!(
        parse_standard_api_baseline(&(header.to_owned() + &oversized_entry)),
        Err(ApiBaselineError::InvalidEntry)
    );

    let mut oversized_inventory = header.to_owned();
    for identity in 0..1_025 {
        let _ = writeln!(
            oversized_inventory,
            "primitive:{identity}\tPrimitive\tPop.Internal\tPop\tBoolean{identity}\tBoolean{identity}\tprelude\timplemented\ttrue\tarchitecture/02-language-model.md"
        );
    }
    assert_eq!(
        parse_standard_api_baseline(&oversized_inventory),
        Err(ApiBaselineError::InvalidEntry)
    );

    let oversized_file = format!("{header}{}", "A".repeat(300_000));
    assert_eq!(
        parse_standard_api_baseline(&oversized_file),
        Err(ApiBaselineError::InvalidEntry)
    );
}
