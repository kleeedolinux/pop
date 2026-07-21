use pop_backend_mir_interp::{
    MirCodecError, MirCodecEvent, MirCodecReader, MirCodecWriter, MirInterpreter, MirValue,
};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, BuiltinTypeId, FileId, ModuleId, NamespaceId, ResultCaseId};
use pop_mir::{MirGeneratedCodecMemberId, lower_hir_bubble};
use pop_source::SourceFile;
use pop_types::{IntegerKind, IntegerValue};

#[test]
fn compiler_generated_codec_entries_round_trip_reject_malformed_and_consume_sequential_values() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/codec.pop",
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         public record Payload\n\
             age: UInt32\n\
         end\n\
         public function schema(): Codec.Schema<Payload>\n\
             return PayloadSchema\n\
         end\n",
    )
    .expect("codec source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(7),
        NamespaceId::from_raw(7),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let hir = front_end.hir().expect("codec HIR");
    let hir_adapter = &hir.generated_codec_adapters()[0];
    let encode_symbol = hir_adapter.encode_entry().symbol();
    let decode_symbol = hir_adapter.decode_entry().symbol();
    let base = lower_hir_bubble(hir, front_end.types()).expect("codec MIR");
    let adapter = &base.generated_codec_adapters()[0];
    let target_symbol = adapter.target().symbol();
    let MirGeneratedCodecMemberId::Field(age_field) = adapter.members()[0].member() else {
        panic!("retained record field")
    };
    let interpreter = MirInterpreter::new(&base, front_end.types()).expect("verified codec MIR");
    let integer = |value| {
        MirValue::Integer(
            IntegerValue::parse_decimal(value, IntegerKind::UInt32).expect("UInt32 value"),
        )
    };
    let record = |value| MirValue::Record {
        record: target_symbol,
        fields: vec![(age_field, integer(value))],
    };
    let encoded = [
        MirCodecEvent::RecordStart(1),
        MirCodecEvent::Member {
            ordinal: 0,
            label: "age".to_owned(),
        },
        MirCodecEvent::Integer(
            IntegerValue::parse_decimal("7", IntegerKind::UInt32).expect("UInt32 value"),
        ),
        MirCodecEvent::RecordEnd,
    ];
    let writer = MirCodecWriter::new();
    assert_eq!(
        interpreter.call(
            encode_symbol,
            &[record("7"), MirValue::CodecWriter(writer.clone())]
        ),
        Ok(vec![MirValue::Result {
            definition: BuiltinTypeId::from_raw(100),
            case: ResultCaseId::from_raw(0),
            arguments: vec![MirValue::Nil],
        }])
    );
    assert_eq!(writer.events(), encoded);
    assert_eq!(
        interpreter.call(
            decode_symbol,
            &[MirValue::CodecReader(MirCodecReader::new(writer.events()))]
        ),
        Ok(vec![MirValue::Result {
            definition: BuiltinTypeId::from_raw(100),
            case: ResultCaseId::from_raw(0),
            arguments: vec![record("7")],
        }])
    );

    assert_eq!(
        interpreter.call(
            encode_symbol,
            &[
                MirValue::Record {
                    record: target_symbol,
                    fields: Vec::new(),
                },
                MirValue::CodecWriter(writer.clone()),
            ]
        ),
        Ok(vec![MirValue::Result {
            definition: BuiltinTypeId::from_raw(100),
            case: ResultCaseId::from_raw(1),
            arguments: vec![MirValue::CodecError(MirCodecError::CapabilityFailure)],
        }])
    );
    assert_eq!(writer.events(), encoded, "failed encode must be atomic");

    assert_eq!(
        interpreter.call(
            encode_symbol,
            &[record("8"), MirValue::CodecWriter(writer.clone())]
        ),
        Ok(vec![MirValue::Result {
            definition: BuiltinTypeId::from_raw(100),
            case: ResultCaseId::from_raw(0),
            arguments: vec![MirValue::Nil],
        }])
    );
    let sequential_reader = MirValue::CodecReader(MirCodecReader::new(writer.events()));
    for value in ["7", "8"] {
        assert_eq!(
            interpreter.call(decode_symbol, std::slice::from_ref(&sequential_reader)),
            Ok(vec![MirValue::Result {
                definition: BuiltinTypeId::from_raw(100),
                case: ResultCaseId::from_raw(0),
                arguments: vec![record(value)],
            }])
        );
    }

    let malformed = MirValue::CodecReader(MirCodecReader::new(vec![
        MirCodecEvent::RecordStart(1),
        MirCodecEvent::Member {
            ordinal: 0,
            label: "wrong".to_owned(),
        },
        MirCodecEvent::Integer(
            IntegerValue::parse_decimal("7", IntegerKind::UInt32).expect("UInt32 value"),
        ),
        MirCodecEvent::RecordEnd,
    ]));
    assert_eq!(
        interpreter.call(decode_symbol, &[malformed]),
        Ok(vec![MirValue::Result {
            definition: BuiltinTypeId::from_raw(100),
            case: ResultCaseId::from_raw(1),
            arguments: vec![MirValue::CodecError(MirCodecError::MalformedInput)],
        }])
    );

    let events = ["7", "8"]
        .into_iter()
        .flat_map(|value| {
            [
                MirCodecEvent::RecordStart(1),
                MirCodecEvent::Member {
                    ordinal: 0,
                    label: "age".to_owned(),
                },
                match integer(value) {
                    MirValue::Integer(value) => MirCodecEvent::Integer(value),
                    _ => unreachable!(),
                },
                MirCodecEvent::RecordEnd,
            ]
        })
        .collect();
    let reader = MirValue::CodecReader(MirCodecReader::new(events));
    let expected = |value| {
        vec![MirValue::Result {
            definition: BuiltinTypeId::from_raw(100),
            case: ResultCaseId::from_raw(0),
            arguments: vec![record(value)],
        }]
    };

    assert_eq!(
        interpreter.call(decode_symbol, std::slice::from_ref(&reader)),
        Ok(expected("7"))
    );
    assert_eq!(
        interpreter.call(decode_symbol, std::slice::from_ref(&reader)),
        Ok(expected("8"))
    );
}
