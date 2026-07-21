use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, ReferenceMetadataDecodeError, ReferenceType,
    analyze_bubble, decode_reference_metadata, encode_reference_metadata,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{MirVerificationError, lower_hir_bubble, parse_mir_dump, verify_mir_bubble};
use pop_source::SourceFile;

fn analyze(text: &str) -> pop_driver::FrontEndResult {
    let source = SourceFile::new(FileId::from_raw(0), "src/checkedCasts.pop", text)
        .expect("checked-cast source");
    analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ))
}

fn analyze_modules(
    bubble: BubbleId,
    dependencies: Vec<BubbleId>,
    modules: Vec<(u32, &str, &str)>,
) -> pop_driver::FrontEndResult {
    let modules = modules
        .into_iter()
        .map(|(id, path, text)| {
            FrontEndModule::new(
                ModuleId::from_raw(id),
                SourceFile::new(FileId::from_raw(id), path, text).expect("checked-cast source"),
            )
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
fn interface_to_class_target_call_reaches_checked_downcast_mir() {
    let result = analyze(
        "namespace Main\n\
         public interface Reader\n\
             function read(): Int\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(): Int\n\
                 return 1\n\
             end\n\
         end\n\
         public function cast(reader: Reader): FileReader?\n\
             return FileReader(reader)\n\
         end\n",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("verified checked-cast HIR");
    let hir_dump = hir.dump(result.types());
    assert!(
        hir_dump.contains("cast.checked-nominal"),
        "expected checked nominal cast in HIR:\n{hir_dump}"
    );
    assert_eq!(
        hir_dump.matches("cast.checked-nominal").count(),
        1,
        "one source operand must produce one cast"
    );

    let mir = lower_hir_bubble(hir, result.types()).expect("verified checked-downcast MIR");
    let mir_dump = mir.dump();
    assert!(
        mir_dump.contains("checkedDowncast"),
        "expected checkedDowncast in MIR:\n{mir_dump}"
    );
    assert_eq!(
        mir_dump.matches("checkedDowncast").count(),
        1,
        "the operand must not be duplicated during lowering"
    );
    let reparsed = parse_mir_dump(&mir_dump).expect("checked-downcast MIR text round trip");
    assert_eq!(reparsed.dump(), mir_dump);
    verify_mir_bubble(&reparsed, result.types()).expect("verified checked-downcast round trip");

    let checked_line = mir_dump
        .lines()
        .find(|line| line.contains("checkedDowncast"))
        .expect("checked-downcast instruction");
    let forged_line = checked_line.replacen(" i0 ", " i999 ", 1);
    let forged = parse_mir_dump(&mir_dump.replacen(checked_line, &forged_line, 1))
        .expect("structurally valid forged checked-downcast MIR");
    let errors = verify_mir_bubble(&forged, result.types()).expect_err("forged cast must fail");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, MirVerificationError::InvalidCheckedDowncast { .. })),
        "expected checked-downcast verifier error: {errors:?}"
    );

    let class_line = mir_dump
        .lines()
        .find(|line| line.starts_with("type.class "))
        .expect("class descriptor");
    let cyclic =
        parse_mir_dump(&mir_dump.replacen(class_line, &format!("{class_line} base c0"), 1))
            .expect("structurally valid cyclic class descriptor");
    let errors = verify_mir_bubble(&cyclic, result.types()).expect_err("cycle must fail closed");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, MirVerificationError::InvalidClassAncestry { .. }))
    );
}

#[test]
fn checked_nominal_cast_reports_the_reserved_typed_diagnostics() {
    let cases = [
        (
            "namespace Main\n\
             public interface Reader\n\
                 function read(): Int\n\
             end\n\
             public function cast(reader: Reader): Reader?\n\
                 return Reader(reader)\n\
             end\n",
            "POP2032",
        ),
        (
            "namespace Main\n\
             public interface Reader\n\
                 function read(): Int\n\
             end\n\
             public class FileReader implements Reader\n\
                 public function FileReader:read(): Int\n\
                     return 1\n\
                 end\n\
             end\n\
             public function cast(reader: Reader?): FileReader?\n\
                 return FileReader(reader)\n\
             end\n",
            "POP2033",
        ),
        (
            "namespace Main\n\
             public interface Reader\n\
                 function read(): Int\n\
             end\n\
             public interface Closeable\n\
                 function close()\n\
             end\n\
             public class Resource implements Closeable\n\
                 public function Resource:close()\n\
                 end\n\
             end\n\
             public function cast(reader: Reader): Resource?\n\
                 return Resource(reader)\n\
             end\n",
            "POP2034",
        ),
    ];

    for (source, expected_code) in cases {
        let result = analyze(source);
        assert!(result.hir().is_none(), "invalid cast reached HIR");
        assert!(
            result.diagnostic_snapshot().contains(expected_code),
            "expected {expected_code}:\n{}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn fully_applied_generic_checked_cast_keeps_invariant_specialization_identity() {
    let result = analyze(
        "namespace Main\n\
         public interface Reader<T>\n\
             function read(): T\n\
         end\n\
         public class Box<T> implements Reader<T>\n\
             public value: T\n\
             public function Box:read(): T\n\
                 return self.value\n\
             end\n\
         end\n\
         public function cast(reader: Reader<Int>): Box<Int>?\n\
             return Box<Int>(reader)\n\
         end\n",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(result.hir().expect("generic cast HIR"), result.types())
        .expect("generic cast MIR");
    assert_eq!(mir.dump().matches("checkedDowncast").count(), 1);
}

#[test]
fn public_nominal_cast_contract_round_trips_from_producer_to_consumer() {
    let producer_bubble = BubbleId::from_raw(41);
    let producer = analyze_modules(
        producer_bubble,
        Vec::new(),
        vec![
            (
                0,
                "src/contracts.pop",
                "namespace Library.Contracts\n\
                 public interface Reader\n\
                     function read(): Int\n\
                 end\n\
                 public open class FileReader implements Reader\n\
                     public function FileReader:read(): Int\n\
                         return 1\n\
                     end\n\
                 end\n\
                 public function narrow(reader: Reader): FileReader?\n\
                     return FileReader(reader)\n\
                 end\n",
            ),
            (
                1,
                "src/privateReader.pop",
                "namespace Library.Contracts\n\
                 private class HiddenReader implements Reader\n\
                     public function HiddenReader:read(): Int\n\
                         return 2\n\
                     end\n\
                 end\n",
            ),
        ],
    );
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );
    let metadata = producer
        .reference_metadata()
        .expect("public checked-cast reference metadata");
    assert_eq!(metadata.interfaces().len(), 1);
    assert_eq!(metadata.classes().len(), 1);
    assert_eq!(metadata.interfaces()[0].name(), "Reader");
    let class = &metadata.classes()[0];
    assert_eq!(class.name(), "FileReader");
    assert!(class.is_open());
    assert!(class.direct_base().is_none());
    assert_eq!(class.interface_witnesses().len(), 1);

    let encoded = encode_reference_metadata(metadata).expect("canonical reference.metadata");
    let text = std::str::from_utf8(&encoded).expect("reference metadata is canonical UTF-8");
    assert!(!text.contains("HiddenReader"));
    let decoded = decode_reference_metadata(&encoded).expect("verified nominal metadata");

    let consumer_source = SourceFile::new(
        FileId::from_raw(2),
        "src/main.pop",
        "namespace Application\n\
         using Library.Contracts\n\
         public function cast(reader: Reader): FileReader?\n\
             return FileReader(reader)\n\
         end\n",
    )
    .expect("consumer source");
    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(42),
            NamespaceId::from_raw(42),
            vec![producer_bubble],
            vec![FrontEndModule::new(ModuleId::from_raw(2), consumer_source)],
        )
        .with_reference_metadata(vec![decoded]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(consumer.hir().expect("consumer HIR"), consumer.types())
        .expect("consumer MIR");
    let dump = mir.dump();
    assert_eq!(dump.matches("checkedDowncast").count(), 1);
    assert!(dump.contains("nominal.interface b41:"), "{dump}");
    assert!(dump.contains("nominal.class b41:"), "{dump}");
    let reparsed = parse_mir_dump(&dump).expect("consumer nominal MIR text round trip");
    assert_eq!(reparsed.dump(), dump);
    verify_mir_bubble(&reparsed, consumer.types()).expect("verified consumer nominal catalog");
}

#[test]
fn generic_nominal_reference_metadata_preserves_exact_witness_arguments() {
    let producer_bubble = BubbleId::from_raw(71);
    let producer = analyze_modules(
        producer_bubble,
        Vec::new(),
        vec![(
            0,
            "src/contracts.pop",
            "namespace Library.Contracts\n\
             public interface Reader<T>\n\
                 function read(): T\n\
             end\n\
             public class Box<T> implements Reader<T>\n\
                 public value: T\n\
                 public function Box:read(): T\n\
                     return self.value\n\
                 end\n\
             end\n",
        )],
    );
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );
    let metadata = producer.reference_metadata().expect("generic metadata");
    let [witness] = metadata.classes()[0].interface_witnesses() else {
        panic!("one exact generic interface witness");
    };
    assert_eq!(witness.arguments(), [ReferenceType::TypeParameter(0)]);
    let decoded = decode_reference_metadata(
        &encode_reference_metadata(metadata).expect("canonical generic metadata"),
    )
    .expect("verified generic metadata");

    let consumer_source = SourceFile::new(
        FileId::from_raw(1),
        "src/main.pop",
        "namespace Application\n\
         using Library.Contracts\n\
         public function cast(reader: Reader<Int>): Box<Int>?\n\
             return Box<Int>(reader)\n\
         end\n",
    )
    .expect("consumer source");
    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(72),
            NamespaceId::from_raw(72),
            vec![producer_bubble],
            vec![FrontEndModule::new(ModuleId::from_raw(1), consumer_source)],
        )
        .with_reference_metadata(vec![decoded]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(
        consumer.hir().expect("generic consumer HIR"),
        consumer.types(),
    )
    .expect("generic consumer MIR");
    assert_eq!(mir.dump().matches("checkedDowncast").count(), 1);
}

#[test]
fn nominal_reference_metadata_fails_closed_when_the_owner_is_tampered() {
    let producer = analyze_modules(
        BubbleId::from_raw(51),
        Vec::new(),
        vec![(
            0,
            "src/contracts.pop",
            "namespace Library.Contracts\n\
             public interface Reader\n\
                 function read(): Int\n\
             end\n\
             public class FileReader implements Reader\n\
                 public function FileReader:read(): Int\n\
                     return 1\n\
                 end\n\
             end\n\
             public function narrow(reader: Reader): FileReader?\n\
                 return FileReader(reader)\n\
             end\n",
        )],
    );
    assert!(producer.diagnostics().is_empty());
    let encoded = encode_reference_metadata(
        producer
            .reference_metadata()
            .expect("nominal reference metadata"),
    )
    .expect("canonical metadata");
    let tampered = String::from_utf8(encoded)
        .expect("UTF-8 metadata")
        .replacen("\"bubble\":51", "\"bubble\":52", 1)
        .into_bytes();
    assert!(matches!(
        decode_reference_metadata(&tampered),
        Err(ReferenceMetadataDecodeError::InvalidNominalMetadata)
    ));
}

#[test]
fn specialized_direct_base_metadata_reaches_the_consumer_mir_exactly() {
    let producer_bubble = BubbleId::from_raw(91);
    let producer = analyze_modules(
        producer_bubble,
        Vec::new(),
        vec![(
            0,
            "src/contracts.pop",
            "namespace Library.Contracts\n\
             public interface Reader<T>\n\
                 function read(): T\n\
             end\n\
             public open class Base<T> implements Reader<T>\n\
                 public value: T\n\
                 public function Base:read(): T\n\
                     return self.value\n\
                 end\n\
             end\n\
             public class Derived<T> implements Reader<T>\n\
                 public value: T\n\
                 public function Derived:read(): T\n\
                     return self.value\n\
                 end\n\
             end\n",
        )],
    );
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );
    let metadata = producer.reference_metadata().expect("producer metadata");
    let base = metadata
        .classes()
        .iter()
        .find(|class| class.name() == "Base")
        .expect("base class");
    let mut encoded = String::from_utf8(
        encode_reference_metadata(metadata).expect("canonical producer metadata"),
    )
    .expect("UTF-8 metadata");
    let marker = "\"name\":\"Derived\",\"type_parameter_count\":1,\"is_open\":false,\"interface_witnesses\":";
    let replacement = format!(
        "\"name\":\"Derived\",\"type_parameter_count\":1,\"is_open\":false,\"direct_base\":{{\"definition\":{{\"bubble\":{},\"symbol\":{}}},\"arguments\":[{{\"TypeParameter\":0}}]}},\"interface_witnesses\":",
        base.identity().bubble().raw(),
        base.identity().symbol().raw(),
    );
    assert!(
        encoded.contains(marker),
        "unexpected metadata shape: {encoded}"
    );
    encoded = encoded.replacen(marker, &replacement, 1);
    let metadata =
        decode_reference_metadata(encoded.as_bytes()).expect("typed direct-base metadata");

    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(92),
            NamespaceId::from_raw(92),
            vec![producer_bubble],
            vec![FrontEndModule::new(
                ModuleId::from_raw(1),
                SourceFile::new(
                    FileId::from_raw(1),
                    "src/main.pop",
                    "namespace Application\n\
                     using Library.Contracts\n\
                     private function retainType(value: Derived<Int>): Derived<Int>\n\
                         return value\n\
                     end\n\
                     public function cast(reader: Reader<Int>): Base<Int>?\n\
                         return Base<Int>(reader)\n\
                     end\n",
                )
                .expect("consumer source"),
            )],
        )
        .with_reference_metadata(vec![metadata]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(consumer.hir().expect("consumer HIR"), consumer.types())
        .expect("consumer MIR");
    let derived = mir
        .nominal_references()
        .classes()
        .iter()
        .find(|reference| reference.identity().definition().symbol() != base.identity().symbol())
        .expect("derived specialization");
    let base = mir
        .nominal_references()
        .classes()
        .iter()
        .find(|reference| reference.identity().definition().symbol() == base.identity().symbol())
        .expect("base specialization");
    assert_eq!(derived.base(), Some(base.class()));
    assert_eq!(derived.base_type(), Some(base.type_id()));
    verify_mir_bubble(&mir, consumer.types()).expect("verified exact specialized ancestry");
}

#[test]
fn nominal_reference_metadata_rejects_a_missing_interface_witness_definition() {
    let producer = analyze_modules(
        BubbleId::from_raw(61),
        Vec::new(),
        vec![(
            0,
            "src/contracts.pop",
            "namespace Library.Contracts\n\
             public interface Reader\n\
                 function read(): Int\n\
             end\n\
             public class FileReader implements Reader\n\
                 public function FileReader:read(): Int\n\
                     return 1\n\
                 end\n\
             end\n",
        )],
    );
    assert!(producer.diagnostics().is_empty());
    let encoded = encode_reference_metadata(
        producer
            .reference_metadata()
            .expect("nominal reference metadata"),
    )
    .expect("canonical metadata");
    let text = String::from_utf8(encoded).expect("UTF-8 metadata");
    let interfaces = text.find("\"interfaces\":[").expect("interface inventory");
    let classes = text[interfaces..]
        .find("],\"classes\":")
        .map(|offset| interfaces + offset)
        .expect("class inventory");
    let mut tampered = text;
    tampered.replace_range(interfaces..classes + 1, "\"interfaces\":[]");
    assert!(matches!(
        decode_reference_metadata(tampered.as_bytes()),
        Err(ReferenceMetadataDecodeError::InvalidNominalMetadata)
    ));
}
