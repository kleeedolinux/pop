use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, RetainedAdapterKind, RetainedMetadataError,
    RetainedProjectionType, analyze_bubble, decode_reference_metadata,
    decode_retained_adapters_popc, encode_reference_metadata,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{MirInstructionKind, lower_hir_bubble, parse_mir_dump, verify_mir_bubble};
use pop_source::SourceFile;
use pop_types::Effect;

fn analyze(source_text: &str) -> pop_driver::FrontEndResult {
    let source = SourceFile::new(FileId::from_raw(0), "src/user.pop", source_text)
        .expect("retained metadata source");
    analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(7),
        NamespaceId::from_raw(7),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ))
}

#[test]
fn trusted_retained_metadata_accepts_the_closed_codec_request() {
    let result = analyze(
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         public record User\n\
             name: String\n\
             age: UInt32\n\
         end\n",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{} {:#?}",
        result.diagnostic_snapshot(),
        result.diagnostics()
    );
    assert!(result.hir().is_some());

    let artifacts = result
        .retained_metadata()
        .expect("verified retained metadata artifacts");
    assert_eq!(artifacts.adapters().len(), 1);
    let adapter = &artifacts.adapters()[0];
    assert_eq!(adapter.target_name(), "User");
    assert_eq!(adapter.adapter_name(), "UserSchema");
    assert_eq!(adapter.kind(), RetainedAdapterKind::Record);
    assert_eq!(adapter.schema_version(), 1);
    assert_eq!(adapter.projection_sha256().len(), 64);
    assert_eq!(adapter.members().len(), 2);
    assert_eq!(adapter.members()[0].name(), "name");
    assert_eq!(
        adapter.members()[0].projected_type(),
        &RetainedProjectionType::String
    );
    assert_eq!(adapter.members()[1].name(), "age");
    assert_eq!(
        adapter.members()[1].projected_type(),
        &RetainedProjectionType::UInt32
    );

    let text = std::str::from_utf8(artifacts.popc()).expect("typed UTF-8 .popc");
    assert!(text.starts_with("@Metadata.GeneratedAdapters(\n"), "{text}");
    assert!(
        text.contains("namespace Pop.Generated.Metadata\n"),
        "{text}"
    );
    assert!(text.contains("internal record Schema0\n"), "{text}");
    assert!(text.ends_with('\n'));
    assert!(!text.contains("{\""), "retained schema must not be JSON");
    assert_eq!(
        decode_retained_adapters_popc(artifacts.popc()).expect("canonical typed .popc"),
        artifacts.clone()
    );
}

#[test]
fn generated_schema_item_is_typed_and_dead_stripped_by_exact_reachability() {
    let reachable = analyze(
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         public record User\n\
             name: String\n\
         end\n\
         public function schema(): Codec.Schema<User>\n\
             return UserSchema\n\
         end\n",
    );
    assert!(
        reachable.diagnostics().is_empty(),
        "{} {:#?}",
        reachable.diagnostic_snapshot(),
        reachable.diagnostics()
    );
    let reachable_hir = reachable.hir().unwrap_or_else(|| {
        panic!(
            "verified adapter HIR: bubble={:?}, build={:#?}",
            reachable.hir_bubble_error(),
            reachable.hir_build_errors()
        )
    });
    let [adapter] = reachable_hir.generated_codec_adapters() else {
        panic!("one generated typed adapter expected")
    };
    assert_eq!(adapter.name(), "UserSchema");
    assert_eq!(adapter.target_name(), "User");
    assert_eq!(adapter.visibility(), pop_resolve::Visibility::Public);
    assert_eq!(adapter.members().len(), 1);
    assert_eq!(adapter.members()[0].name(), "name");
    assert_eq!(adapter.members()[0].ordinal(), 0);
    assert!(adapter.members()[0].field().is_some());
    assert_eq!(adapter.projection_sha256().len(), 64);
    assert_ne!(
        adapter.encode_entry().symbol(),
        adapter.decode_entry().symbol()
    );
    assert_eq!(adapter.encode_entry().parameters().len(), 2);
    assert_eq!(adapter.decode_entry().parameters().len(), 1);
    assert!(adapter.encode_entry().effects().contains(Effect::Allocates));
    assert!(
        adapter
            .encode_entry()
            .effects()
            .contains(Effect::GcSafePoint)
    );
    assert!(adapter.decode_entry().effects().contains(Effect::Allocates));
    assert!(
        adapter
            .decode_entry()
            .effects()
            .contains(Effect::GcSafePoint)
    );
    let reachable_mir =
        lower_hir_bubble(reachable_hir, reachable.types()).expect("verified adapter MIR");
    assert_eq!(reachable_mir.generated_codec_adapters().len(), 1);
    let encode = reachable_mir
        .functions()
        .iter()
        .find(|function| function.symbol() == adapter.encode_entry().symbol())
        .expect("ordinary generated encode MIR function");
    assert!(encode.blocks().iter().flat_map(|block| block.instructions()).any(|instruction| {
        matches!(instruction.kind(), MirInstructionKind::CodecEncode { adapter: found, .. } if *found == adapter.symbol())
    }));
    let decode = reachable_mir
        .functions()
        .iter()
        .find(|function| function.symbol() == adapter.decode_entry().symbol())
        .expect("ordinary generated decode MIR function");
    assert!(decode.blocks().iter().flat_map(|block| block.instructions()).any(|instruction| {
        matches!(instruction.kind(), MirInstructionKind::CodecDecode { adapter: found, .. } if *found == adapter.symbol())
    }));
    assert!(reachable_mir.dump().contains("codec.schema"));
    assert!(!reachable_mir.dump().contains("lookup"));

    let unreachable = analyze(
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         private record Hidden\n\
             name: String\n\
         end\n\
         public function run(): Int\n\
             return 1\n\
         end\n",
    );
    assert!(unreachable.diagnostics().is_empty());
    let unreachable_hir = unreachable.hir().expect("verified adapter HIR");
    assert_eq!(unreachable_hir.generated_codec_adapters().len(), 1);
    let encode_symbol = unreachable_hir.generated_codec_adapters()[0]
        .encode_entry()
        .symbol();
    let decode_symbol = unreachable_hir.generated_codec_adapters()[0]
        .decode_entry()
        .symbol();
    let unreachable_mir =
        lower_hir_bubble(unreachable_hir, unreachable.types()).expect("dead-stripped adapter MIR");
    assert!(unreachable_mir.generated_codec_adapters().is_empty());
    assert!(unreachable_mir.functions().iter().all(|function| {
        function.symbol() != encode_symbol && function.symbol() != decode_symbol
    }));
    assert!(!unreachable_mir.dump().contains("codec.schema"));
}

#[test]
fn codec_error_cases_are_source_resolvable_and_exhaustively_matchable() {
    let result = analyze(
        "namespace Example\n\
         public function malformed(): Codec.Error\n\
             return Codec.Error.MalformedInput\n\
         end\n\
         public function classify(error: Codec.Error): Int\n\
             match error\n\
             when Codec.Error.MalformedInput then\n\
                 return 1\n\
             when Codec.Error.LimitExceeded then\n\
                 return 2\n\
             when Codec.Error.CapabilityFailure then\n\
                 return 3\n\
             end\n\
         end\n",
    );
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("verified Codec.Error HIR");
    let hir_dump = hir.dump(result.types());
    assert!(
        hir_dump.contains("codec.error MalformedInput"),
        "{hir_dump}"
    );
    assert!(hir_dump.contains("codec.error.match"), "{hir_dump}");
    let mir = lower_hir_bubble(hir, result.types()).expect("verified Codec.Error MIR");
    let dump = mir.dump();
    assert!(dump.contains("codec.error MalformedInput"));
    assert!(dump.contains("codec.error.discriminant"));
    let reparsed = parse_mir_dump(&dump).expect("Codec.Error MIR text round trip");
    assert_eq!(reparsed.dump(), dump);
    verify_mir_bubble(&reparsed, result.types()).expect("reparsed Codec.Error MIR verifies");
    let duplicate = parse_mir_dump(&dump.replacen("case#2:", "case#1:", 1))
        .expect("structurally valid duplicate Codec.Error arm");
    assert!(verify_mir_bubble(&duplicate, result.types()).is_err());
    assert!(
        parse_mir_dump(&dump.replacen("codec.error MalformedInput", "codec.error Unknown", 1))
            .is_err()
    );

    let incomplete = analyze(
        "namespace Example\n\
         public function classify(error: Codec.Error): Int\n\
             match error\n\
             when Codec.Error.MalformedInput then\n\
                 return 1\n\
             end\n\
         end\n",
    );
    assert!(
        incomplete
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().as_str() == "POP2020"),
        "{}",
        incomplete.diagnostic_snapshot()
    );

    let duplicate = analyze(
        "namespace Example\n\
         public function classify(error: Codec.Error): Int\n\
             match error\n\
             when Codec.Error.MalformedInput then\n\
                 return 1\n\
             when Codec.Error.MalformedInput then\n\
                 return 2\n\
             when Codec.Error.LimitExceeded then\n\
                 return 3\n\
             when Codec.Error.CapabilityFailure then\n\
                 return 4\n\
             end\n\
         end\n",
    );
    assert!(duplicate.hir().is_none());
    assert!(
        duplicate
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().as_str() == "POP2021")
    );

    let foreign = analyze(
        "namespace Example\n\
         public function classify(error: Codec.Error): Int\n\
             match error\n\
             when Result.Ok then\n\
                 return 1\n\
             when Codec.Error.MalformedInput then\n\
                 return 2\n\
             when Codec.Error.LimitExceeded then\n\
                 return 3\n\
             when Codec.Error.CapabilityFailure then\n\
                 return 4\n\
             end\n\
         end\n",
    );
    assert!(foreign.hir().is_none());
    assert!(
        foreign
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().as_str() == "POP2022")
    );

    let shadowed = analyze(
        "namespace Codec\n\
         error Error\n\
             MalformedInput\n\
         end\n\
         function malformed(): Error\n\
             return Codec.Error.MalformedInput\n\
         end\n",
    );
    assert!(
        shadowed.diagnostics().is_empty(),
        "{}",
        shadowed.diagnostic_snapshot()
    );
    assert!(
        !shadowed
            .hir()
            .expect("ordinary shadowing error HIR")
            .dump(shadowed.types())
            .contains("codec.error"),
        "a user Error declaration must not acquire the compiler-known Codec.Error identity"
    );
}

#[test]
fn retained_metadata_rejects_non_data_targets_and_noncanonical_arguments() {
    for source in [
        "namespace Example\n@RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\npublic function run()\nend\n",
        "namespace Example\n@RetainMetadata(schemaVersion = 1, use = Metadata.Use.Codec)\npublic record User\n    name: String\nend\n",
        "namespace Example\n@RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 0)\npublic record User\n    name: String\nend\n",
        "namespace Example\n@RetainMetadata(use = Metadata.Use.Other, schemaVersion = 1)\npublic record User\n    name: String\nend\n",
    ] {
        let result = analyze(source);
        assert!(result.hir().is_none(), "invalid request reached HIR");
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code().as_str() == "POP4001")
        );
    }
}

#[test]
fn retained_metadata_rejects_unsupported_dynamic_shapes() {
    let result = analyze(
        "namespace Example\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         public record User\n\
             attributes: Table<String, String>\n\
         end\n",
    );

    assert!(result.hir().is_none(), "table projection reached HIR");
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().as_str() == "POP4001")
    );
}

#[test]
fn retained_metadata_closes_nested_enum_union_and_container_projections() {
    let result = analyze(
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 3)\n\
         public enum Color\n\
             Red\n\
             Blue\n\
         end\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 4)\n\
         public union Choice\n\
             Named(name: String)\n\
             Numbered(value: UInt32)\n\
         end\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 5)\n\
         public record Envelope\n\
             color: Color?\n\
             choice: Choice\n\
             names: List<String>\n\
             bytes: Bytes\n\
         end\n",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{} {:#?}",
        result.diagnostic_snapshot(),
        result.diagnostics()
    );
    let artifacts = result.retained_metadata().expect("closed projection");
    assert_eq!(artifacts.adapters().len(), 3);
    assert!(artifacts.adapters().iter().all(|adapter| {
        adapter.projection_sha256().len() == 64
            && adapter
                .projection_sha256()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }));
    let envelope = artifacts
        .adapters()
        .iter()
        .find(|adapter| adapter.target_name() == "Envelope")
        .expect("Envelope adapter");
    assert!(matches!(
        envelope.members()[0].projected_type(),
        RetainedProjectionType::Optional(value)
            if matches!(value.as_ref(), RetainedProjectionType::Nominal { .. })
    ));
    assert!(matches!(
        envelope.members()[2].projected_type(),
        RetainedProjectionType::List(value)
            if value.as_ref() == &RetainedProjectionType::String
    ));
    assert_eq!(
        envelope.members()[3].projected_type(),
        &RetainedProjectionType::Bytes
    );
}

#[test]
fn retained_metadata_descriptor_rejects_tamper_json_and_noncanonical_input() {
    let result = analyze(
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         public record User\n\
             name: String\n\
         end\n",
    );
    let artifacts = result.retained_metadata().expect("retained metadata");

    assert_eq!(
        decode_retained_adapters_popc(b"{\"schema\":[]}\n"),
        Err(RetainedMetadataError::InvalidDescriptor)
    );
    let mut tampered = artifacts.popc().to_vec();
    let name = tampered
        .windows(4)
        .position(|window| window == b"name")
        .expect("field label");
    tampered[name] = b'N';
    assert_eq!(
        decode_retained_adapters_popc(&tampered),
        Err(RetainedMetadataError::InvalidDescriptor)
    );
    let mut no_final_newline = artifacts.popc().to_vec();
    no_final_newline.pop();
    assert_eq!(
        decode_retained_adapters_popc(&no_final_newline),
        Err(RetainedMetadataError::InvalidDescriptor)
    );
}

#[test]
fn only_public_adapters_enter_the_public_typed_descriptor_and_reference_inventory() {
    let result = analyze(
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         public record PublicUser\n\
             privateDataLabel: String\n\
         end\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         internal record InternalUser\n\
             name: String\n\
         end\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         private record PrivateUser\n\
             name: String\n\
         end\n",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let artifacts = result.retained_metadata().expect("all private build facts");
    let public_popc = artifacts.public_popc().expect("public typed descriptor");
    let public_text = std::str::from_utf8(&public_popc).expect("typed UTF-8 .popc");
    assert!(public_text.contains("Example.Models.PublicUser"));
    assert!(!public_text.contains("InternalUser"));
    assert!(!public_text.contains("PrivateUser"));

    let metadata = result
        .reference_metadata()
        .expect("public reference metadata");
    let [adapter] = metadata.retained_adapters() else {
        panic!("only one public adapter reference expected")
    };
    assert_eq!(adapter.name(), "PublicUserSchema");
    assert_eq!(adapter.descriptor_path(), "retained-adapters.popc");
    assert_eq!(adapter.descriptor_size(), public_popc.len() as u64);
    assert_eq!(adapter.descriptor_sha256().len(), 64);
    assert_eq!(adapter.projection_sha256().len(), 64);
    let encoded = encode_reference_metadata(metadata).expect("canonical JSON control metadata");
    let json = std::str::from_utf8(&encoded).expect("canonical UTF-8 control metadata");
    assert!(!json.contains("privateDataLabel"));
    assert!(!json.contains("members"));
    assert_eq!(
        decode_reference_metadata(&encoded).expect("adapter-only metadata round trip"),
        metadata.clone()
    );
}
