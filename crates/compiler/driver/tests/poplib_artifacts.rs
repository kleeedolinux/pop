use std::path::{Path, PathBuf};

use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, NativeLinkPlanSource, PoplibEmission, PoplibError,
    analyze_bubble, emit_poplib, load_poplib, resolve_native_link_inputs,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_hir::HirGeneratedCodecEntryRole;
use pop_mir::{MirInstructionKind, lower_hir_bubble};
use pop_projects::{BubbleKind, parse_package_manifest};
use pop_source::SourceFile;
use pop_target::TargetSpec;

const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn module(text: &str) -> FrontEndModule {
    FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(FileId::from_raw(0), "src/lib.pop", text).expect("source"),
    )
}

fn temporary_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pop-poplib-artifact-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove prior artifact fixture");
    }
    std::fs::create_dir_all(&root).expect("create artifact fixture");
    root
}

fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn poplib_emission_round_trips_generic_metadata_and_rejects_corruption() {
    let bubble = BubbleId::from_raw(2);
    let library = analyze_bubble(FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            "namespace Pop.Sequence\n\
             private function privateIdentity<T>(value: T): T\n\
                 return value\n\
             end\n\
             public function identity<T>(value: T): T\n\
                 return privateIdentity(value)\n\
             end\n",
        )],
    ));
    assert!(
        library.diagnostics().is_empty(),
        "{}",
        library.diagnostic_snapshot()
    );
    let root = temporary_root();
    let native_link_plan = parse_package_manifest(
        "[package]\nname = \"Pop.Standard\"\nversion = \"0.1.0\"\nedition = \"2026\"\n[nativeLibraries]\nZlib = { kind = \"system\", name = \"z\" }\n",
    )
    .expect("native requirements")
    .native_link_plan("x86_64-unknown-linux-gnu")
    .expect("native link plan");
    let native_link_resolution = resolve_native_link_inputs(
        &[NativeLinkPlanSource::new(&root, native_link_plan.clone())],
        &TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target"),
    )
    .expect("resolved native provider");
    let emission = PoplibEmission::new(
        "Pop.Standard",
        "0.1.0",
        ZERO_SHA256,
        "Pop.Standard",
        BubbleKind::Library,
        "2026",
        library
            .reference_metadata()
            .expect("reference metadata")
            .clone(),
    )
    .with_native_link_plan(native_link_plan)
    .with_resolved_native_providers(native_link_resolution.providers().to_vec())
    .with_documentation(b"<?xml version=\"1.0\"?><doc/>\n".to_vec())
    .with_target_implementation("x86_64-unknown-linux-gnu", b"opaque-native-object".to_vec());
    let artifact = root.join("Pop.Standard.poplib");

    emit_poplib(&artifact, &emission).expect("verified artifact emission");
    let manifest = bytes(&artifact.join("bubble.manifest"));
    let reference = bytes(&artifact.join("reference.metadata"));
    let loaded = load_poplib(&artifact).expect("verified artifact loading");
    assert_eq!(
        loaded.reference_metadata(),
        library.reference_metadata().expect("reference metadata")
    );
    assert_eq!(
        loaded.documentation(),
        Some(b"<?xml version=\"1.0\"?><doc/>\n".as_slice())
    );
    assert_eq!(
        loaded.target_implementation(),
        Some((
            "x86_64-unknown-linux-gnu",
            b"opaque-native-object".as_slice()
        ))
    );
    assert_eq!(loaded.public_api_sha256().len(), 64);
    assert_eq!(loaded.native_link_plans().len(), 1);
    assert_eq!(loaded.native_link_plans()[0].libraries()[0].alias(), "Zlib");
    assert_eq!(loaded.resolved_native_providers().len(), 1);
    assert_eq!(loaded.resolved_native_providers()[0].identity(), "z");
    assert_eq!(loaded.resolved_native_providers()[0].version(), None);
    assert_eq!(
        loaded.resolved_native_providers()[0].link_libraries(),
        ["z"]
    );

    emit_poplib(&artifact, &emission).expect("deterministic replacement");
    assert_eq!(bytes(&artifact.join("bubble.manifest")), manifest);
    assert_eq!(bytes(&artifact.join("reference.metadata")), reference);

    std::fs::write(
        artifact.join("targets/x86_64-unknown-linux-gnu/native.object"),
        b"corrupt",
    )
    .expect("corrupt target");
    assert_eq!(load_poplib(&artifact), Err(PoplibError::SizeMismatch));

    emit_poplib(&artifact, &emission).expect("restore artifact");
    std::fs::write(artifact.join("unexpected"), b"extra").expect("extra file");
    assert_eq!(load_poplib(&artifact), Err(PoplibError::UnexpectedFile));

    std::fs::remove_dir_all(root).expect("remove artifact fixture");
}

#[test]
fn poplib_inventories_and_verifies_public_typed_retained_adapters() {
    let bubble = BubbleId::from_raw(9);
    let library = analyze_bubble(FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(9),
        Vec::new(),
        vec![module(
            "namespace Example.Models\n\
             @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
             public record User\n\
                 name: String\n\
             end\n\
             @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
             public enum State\n\
                 Ready\n\
                 Closed\n\
             end\n\
             @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
             public union Choice\n\
                 Item(value: Int)\n\
                 Empty\n\
             end\n\
             @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
             private record Secret\n\
                 value: String\n\
             end\n",
        )],
    ));
    assert!(
        library.diagnostics().is_empty(),
        "{}",
        library.diagnostic_snapshot()
    );
    let producer_entries = library
        .hir()
        .expect("producer adapter HIR")
        .generated_codec_adapters()
        .iter()
        .filter(|adapter| adapter.visibility() == pop_resolve::Visibility::Public)
        .map(|adapter| {
            (
                adapter.name().to_owned(),
                (
                    adapter.encode_entry().identity(),
                    adapter.decode_entry().identity(),
                    adapter.provenance(),
                ),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let public_popc = library
        .retained_metadata()
        .expect("retained metadata")
        .public_popc()
        .expect("public descriptor");
    let emission = PoplibEmission::new(
        "Example.Models",
        "1.0.0",
        ZERO_SHA256,
        "Models",
        BubbleKind::Library,
        "2026",
        library
            .reference_metadata()
            .expect("reference metadata")
            .clone(),
    )
    .with_retained_adapters_popc(public_popc.clone());
    let root = temporary_root();
    let artifact = root.join("Models.poplib");

    emit_poplib(&artifact, &emission).expect("typed adapter artifact emission");
    assert_eq!(bytes(&artifact.join("retained-adapters.popc")), public_popc);
    assert!(!String::from_utf8_lossy(&public_popc).contains("Secret"));
    let loaded = load_poplib(&artifact).expect("typed adapter artifact load");
    assert_eq!(
        loaded.retained_adapters_popc(),
        Some(public_popc.as_slice())
    );
    assert_eq!(loaded.reference_metadata().retained_adapters().len(), 3);
    assert_eq!(loaded.reference_metadata().records().len(), 0);
    let consumer_source = "namespace Consumer\n\
         public function userSchema(): Codec.Schema<Example.Models.User>\n\
             return Example.Models.UserSchema\n\
         end\n\
         public function stateSchema(): Codec.Schema<Example.Models.State>\n\
             return Example.Models.StateSchema\n\
         end\n\
         public function choiceSchema(): Codec.Schema<Example.Models.Choice>\n\
             return Example.Models.ChoiceSchema\n\
         end\n";
    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![bubble],
            vec![module(consumer_source)],
        )
        .with_reference_metadata(vec![loaded.reference_metadata().clone()])
        .with_reference_retained_adapters_popc(vec![(
            bubble,
            loaded
                .retained_adapters_popc()
                .expect("loaded public descriptor")
                .to_vec(),
        )]),
    );
    assert!(
        consumer.diagnostics().is_empty() && consumer.hir().is_some(),
        "{} {:?} {:#?}",
        consumer.diagnostic_snapshot(),
        consumer.hir_bubble_error(),
        consumer.hir_build_errors(),
    );
    let consumer_hir = consumer.hir().expect("source-free consumer HIR");
    assert_eq!(consumer_hir.generated_codec_adapters().len(), 3);
    let entry_symbols = consumer_hir
        .generated_codec_adapters()
        .iter()
        .map(|adapter| {
            let producer = producer_entries
                .get(adapter.name())
                .expect("producer public typed entries");
            assert_eq!(adapter.encode_entry().identity(), producer.0);
            assert_eq!(adapter.decode_entry().identity(), producer.1);
            assert_eq!(adapter.provenance(), producer.2);
            assert_eq!(adapter.encode_entry().identity().adapter().bubble(), bubble);
            assert_eq!(adapter.decode_entry().identity().adapter().bubble(), bubble);
            assert_eq!(
                adapter.encode_entry().identity().role(),
                HirGeneratedCodecEntryRole::Encode
            );
            assert_eq!(
                adapter.decode_entry().identity().role(),
                HirGeneratedCodecEntryRole::Decode
            );
            assert_eq!(adapter.encode_entry().parameters().len(), 2);
            assert_eq!(adapter.decode_entry().parameters().len(), 1);
            (
                adapter.symbol(),
                adapter.encode_entry().symbol(),
                adapter.decode_entry().symbol(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        consumer_hir
            .generated_codec_adapters()
            .iter()
            .map(|adapter| (adapter.name(), adapter.members().len()))
            .collect::<std::collections::BTreeMap<_, _>>(),
        std::collections::BTreeMap::from([
            ("ChoiceSchema", 2),
            ("StateSchema", 2),
            ("UserSchema", 1),
        ])
    );
    let consumer_mir =
        lower_hir_bubble(consumer_hir, consumer.types()).expect("source-free imported adapter MIR");
    assert_eq!(consumer_mir.generated_codec_adapters().len(), 3);
    for (adapter, encode, decode) in entry_symbols {
        let encode = consumer_mir
            .functions()
            .iter()
            .find(|function| function.symbol() == encode)
            .expect("reconstructed encode entry MIR");
        assert!(encode.blocks().iter().flat_map(|block| block.instructions()).any(
            |instruction| matches!(instruction.kind(), MirInstructionKind::CodecEncode { adapter: found, .. } if *found == adapter)
        ));
        let decode = consumer_mir
            .functions()
            .iter()
            .find(|function| function.symbol() == decode)
            .expect("reconstructed decode entry MIR");
        assert!(decode.blocks().iter().flat_map(|block| block.instructions()).any(
            |instruction| matches!(instruction.kind(), MirInstructionKind::CodecDecode { adapter: found, .. } if *found == adapter)
        ));
    }

    let missing_descriptor = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(11),
            NamespaceId::from_raw(11),
            vec![bubble],
            vec![module(consumer_source)],
        )
        .with_reference_metadata(vec![loaded.reference_metadata().clone()]),
    );
    assert!(
        missing_descriptor.hir().is_none(),
        "public adapter structure must never fall back to JSON reference metadata"
    );

    let mut wrong_descriptor = public_popc.clone();
    let wrong_index = wrong_descriptor
        .windows(4)
        .position(|window| window == b"name")
        .expect("field label");
    wrong_descriptor[wrong_index] = b'N';
    let mismatched_descriptor = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(12),
            NamespaceId::from_raw(12),
            vec![bubble],
            vec![module(consumer_source)],
        )
        .with_reference_metadata(vec![loaded.reference_metadata().clone()])
        .with_reference_retained_adapters_popc(vec![(bubble, wrong_descriptor)]),
    );
    assert!(
        mismatched_descriptor.hir().is_none(),
        "descriptor bytes must match the full reference-metadata digest"
    );

    let mut tampered = public_popc;
    let index = tampered
        .windows(4)
        .position(|window| window == b"name")
        .expect("field label");
    tampered[index] = b'N';
    std::fs::write(artifact.join("retained-adapters.popc"), tampered).expect("tamper descriptor");
    assert!(matches!(
        load_poplib(&artifact),
        Err(PoplibError::HashMismatch | PoplibError::InvalidRetainedMetadata)
    ));

    std::fs::remove_dir_all(root).expect("remove artifact fixture");
}
