use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, ReferenceMetadataDecodeError, analyze_bubble,
    decode_reference_metadata, encode_reference_metadata,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::lower_hir_bubble;
use pop_source::SourceFile;

fn analyze(text: &str) -> pop_driver::FrontEndResult {
    let source = SourceFile::new(FileId::from_raw(0), "src/views.pop", text).expect("view source");
    analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ))
}

fn remove_first_lifetime_summary(encoded: &[u8]) -> Vec<u8> {
    let text = String::from_utf8(encoded.to_vec()).expect("reference metadata is UTF-8");
    let needle = ",\"lifetime_summary\":";
    let start = text.find(needle).expect("emitted lifetime summary");
    let value_start = start + needle.len();
    let mut depth = 0_u32;
    let mut end = None;
    for (offset, byte) in text.as_bytes()[value_start..].iter().copied().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(value_start + offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.expect("complete lifetime summary object");
    format!("{}{}", &text[..start], &text[end..]).into_bytes()
}

#[test]
fn exact_view_surface_preserves_parameter_lender_provenance() {
    let result = analyze(
        "namespace Main\n\
         public function bytesPart(bytes: Bytes): Bytes.View\n\
             local whole = Bytes.view(bytes)\n\
             local part = Bytes.slice(whole, 1, 2)\n\
             return part\n\
         end\n\
         public function textPart(text: String): Text.View\n\
             if Text.length(Text.view(text)) == 0 then\n\
                 return Text.slice(text, 1, 0)\n\
             else\n\
                 return Text.slice(Text.view(text), 1, 1)\n\
             end\n\
         end\n\
         private function inspect(view: Bytes.View): Int\n\
             return Bytes.length(view)\n\
         end\n\
         public function consume(bytes: Bytes): Int\n\
             local view = Bytes.view(bytes)\n\
             local present: Byte? = Bytes.get(view, 1)\n\
             local copy: Bytes = Bytes.toBytes(view)\n\
             return inspect(Bytes.slice(copy, 1, 0))\n\
         end\n",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{}\n{:#?}",
        result.diagnostic_snapshot(),
        result.diagnostics()
    );
    let hir = result.hir().unwrap_or_else(|| {
        panic!(
            "view HIR: bubble={:?} build={:#?} type14={:?} type15={:?}",
            result.hir_bubble_error(),
            result.hir_build_errors(),
            result.types().get(pop_foundation::TypeId::from_raw(14)),
            result.types().get(pop_foundation::TypeId::from_raw(15))
        )
    });
    let hir_dump = hir.dump(result.types());
    for operation in [
        "view.create",
        "view.slice",
        "view.length",
        "view.get-byte",
        "view.materialize",
    ] {
        assert!(
            hir_dump.contains(operation),
            "missing {operation}:\n{hir_dump}"
        );
    }
    for function in hir.functions() {
        assert!(
            function
                .lifetime_summary()
                .is_canonical_for(function.parameters().len(), function.results().len())
        );
    }
    let mir = lower_hir_bubble(hir, result.types()).expect("view MIR");
    let dump = mir.dump();
    for operation in [
        "viewCreate",
        "viewSlice",
        "viewLength",
        "viewGetByte",
        "viewMaterialize",
        "viewEnd",
    ] {
        assert!(dump.contains(operation), "missing {operation}:\n{dump}");
    }
}

#[test]
fn views_fail_closed_for_escape_retention_suspension_and_ffi() {
    let cases = [
        (
            "namespace Main\npublic function bad(): Text.View\n    local text = \"local\"\n    return Text.view(text)\nend\n",
            "POP2035",
        ),
        (
            "namespace Main\nprivate record Box\n    value: Bytes.View\nend\npublic function bad(bytes: Bytes): Box\n    return { value = Bytes.view(bytes) }\nend\n",
            "POP2035",
        ),
        (
            "namespace Main\nprivate function retain(value: Bytes.View): Int\n    local observe = function(): Int\n        return Bytes.length(value)\n    end\n    return 0\nend\npublic function bad(bytes: Bytes): Int\n    return retain(Bytes.view(bytes))\nend\n",
            "POP2036",
        ),
        (
            "namespace Main\nprivate async function ready(): Int\n    return 1\nend\npublic async function bad(bytes: Bytes): Int\n    local view = Bytes.view(bytes)\n    local value = await ready()\n    return Bytes.length(view) + value\nend\n",
            "POP2037",
        ),
        (
            "namespace Main\npublic function bad(value: function(view: Bytes.View): Int): Int\n    return 0\nend\n",
            "POP2038",
        ),
    ];

    for (case_index, (source, code)) in cases.into_iter().enumerate() {
        let result = analyze(source);
        assert!(
            result.hir().is_none(),
            "invalid view program {case_index} reached HIR"
        );
        assert!(
            result.diagnostic_snapshot().contains(code),
            "case {case_index} expected {code}:\n{}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn reference_metadata_round_trips_exact_summaries_and_fails_closed_for_views() {
    let borrowed = analyze(
        "namespace Main\npublic function part(text: String, count: Int): Text.View\n    return Text.view(text)\nend\n",
    );
    assert!(
        borrowed.diagnostics().is_empty(),
        "{}",
        borrowed.diagnostic_snapshot()
    );
    let metadata = borrowed
        .reference_metadata()
        .expect("borrowed reference metadata");
    let encoded = encode_reference_metadata(metadata).expect("encode borrowed metadata");
    let text = String::from_utf8(encoded.clone()).expect("reference metadata is UTF-8");
    assert!(text.contains("\"proof_version\":1"), "{text}");
    assert!(text.contains("\"ReturnsAlias\":0"), "{text}");
    let decoded = decode_reference_metadata(&encoded).expect("exact summary round trip");
    assert_eq!(
        encode_reference_metadata(&decoded).expect("canonical re-encoding"),
        encoded
    );

    assert_eq!(
        decode_reference_metadata(&remove_first_lifetime_summary(&encoded)),
        Err(ReferenceMetadataDecodeError::InvalidLifetimeSummary)
    );
    let wrong_version = text
        .replacen("\"proof_version\":1", "\"proof_version\":2", 1)
        .into_bytes();
    assert_eq!(
        decode_reference_metadata(&wrong_version),
        Err(ReferenceMetadataDecodeError::InvalidLifetimeSummary)
    );
    let wrong_parameter = text
        .replacen("\"ReturnsAlias\":0", "\"ReturnsAlias\":9", 1)
        .into_bytes();
    assert_eq!(
        decode_reference_metadata(&wrong_parameter),
        Err(ReferenceMetadataDecodeError::InvalidLifetimeSummary)
    );
    let wrong_parameter_type = text
        .replacen("\"ReturnsAlias\":0", "\"ReturnsAlias\":1", 1)
        .into_bytes();
    assert_eq!(
        decode_reference_metadata(&wrong_parameter_type),
        Err(ReferenceMetadataDecodeError::InvalidLifetimeSummary)
    );

    let ordinary = analyze(
        "namespace Main\npublic function identity(value: Int): Int\n    return value\nend\n",
    );
    let ordinary_encoded = encode_reference_metadata(
        ordinary
            .reference_metadata()
            .expect("ordinary reference metadata"),
    )
    .expect("encode ordinary metadata");
    let missing_ordinary = remove_first_lifetime_summary(&ordinary_encoded);
    let decoded_ordinary = decode_reference_metadata(&missing_ordinary)
        .expect("missing ordinary summary uses conservative compatibility facts");
    assert!(decoded_ordinary.functions()[0].lifetime_summary().is_none());

    let changed_source = analyze(
        "namespace Main\npublic function part(first: String, second: String): Text.View\n    return Text.view(second)\nend\n",
    );
    let changed = encode_reference_metadata(
        changed_source
            .reference_metadata()
            .expect("changed borrowed metadata"),
    )
    .expect("encode changed borrowed metadata");
    assert_ne!(
        encoded, changed,
        "summary/source changes alter canonical API bytes"
    );
}
