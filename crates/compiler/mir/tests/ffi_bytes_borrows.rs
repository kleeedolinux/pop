use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{lower_hir_bubble, optimize_mir, parse_mir_dump, verify_mir_bubble};
use pop_source::SourceFile;

#[test]
fn immutable_bytes_borrow_round_trips_and_rejects_corrupt_region_plans() {
    let ffi = BubbleId::from_raw(20);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/withPin.pop",
        "namespace Memory\n\
         public function inspect(bytes: Bytes): Boolean\n\
             return Ffi.withPin(bytes, function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Boolean\n\
                 return Ffi.OptionalReadOnlyPointer.isPresent(pointer)\n\
             end)\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(front_end.hir().expect("byte pin HIR"), front_end.types())
        .expect("verified byte pin MIR");
    let dump = mir.dump();
    let reparsed = parse_mir_dump(&dump).expect("byte borrow MIR round trip");
    assert_eq!(reparsed.dump(), dump);
    assert_eq!(verify_mir_bubble(&reparsed, front_end.types()), Ok(()));

    assert_corruptions_rejected(&dump, front_end.types());

    let optimized = optimize_mir(mir, front_end.types()).expect("optimized byte borrow MIR");
    let optimized_dump = optimized.dump();
    for operation in [
        "ffiBytesBorrow",
        "ffiBytesBorrowLength",
        "callScopedBorrow",
        "ffiBytesEndBorrow",
    ] {
        assert!(
            optimized_dump.contains(operation),
            "optimizer removed {operation}:\n{optimized_dump}"
        );
    }
}

fn assert_corruptions_rejected(dump: &str, types: &pop_types::TypeArena) {
    let borrow = line_containing(dump, "ffiBytesBorrow v");
    let length = line_containing(dump, "ffiBytesBorrowLength");
    let call = line_containing(dump, "callScopedBorrow");
    let end = line_containing(dump, "ffiBytesEndBorrow");
    let pointer = borrow
        .trim_start()
        .split_once(':')
        .expect("borrow result")
        .0;
    let pointer_type = borrow
        .split_once(':')
        .and_then(|(_, rest)| rest.split_once(" ="))
        .expect("borrow result type")
        .0;
    let owner = borrow
        .split_once("ffiBytesBorrow ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .expect("Bytes owner");
    let borrowed_length = length
        .trim_start()
        .split_once(':')
        .expect("length result")
        .0;
    let region = borrow.split_once("region#").expect("borrow region").1;
    let boolean = types.source_type("Boolean").expect("Boolean");

    let corruptions = [
        dump.replace(&format!("{length}\n"), ""),
        dump.replace(&format!("{call}\n"), ""),
        dump.replace(&format!("{end}\n"), ""),
        dump.replace(
            length,
            &length.replace(&format!("region#{region}"), "region#999"),
        ),
        dump.replace(
            &format!("ffiBytesBorrowLength {owner}"),
            &format!("ffiBytesBorrowLength {pointer}"),
        ),
        dump.replace(
            &format!("({pointer}, {borrowed_length})"),
            &format!("({borrowed_length}, {pointer})"),
        ),
        dump.replace(
            call,
            &format!(
                "    v999:t{} = ffiPointerIsPresent {pointer}\n{call}",
                boolean.raw()
            ),
        ),
        dump.replace(
            call,
            &format!("    v998:{pointer_type} = ffiBytesBorrow {owner} region#999\n{call}"),
        ),
        dump.replace(&format!("{call}\n{end}"), &format!("{end}\n{call}")),
        dump.replace(
            &format!("{pointer_type} = ffiBytesBorrow"),
            &format!("t{} = ffiBytesBorrow", boolean.raw()),
        ),
    ];
    for (index, corruption) in corruptions.into_iter().enumerate() {
        assert_ne!(corruption, dump, "corruption did not alter the MIR dump");
        assert_invalid(index, &corruption, types);
    }
}

fn line_containing<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("missing {needle}:\n{text}"))
}

fn assert_invalid(index: usize, text: &str, types: &pop_types::TypeArena) {
    let corrupted = parse_mir_dump(text)
        .unwrap_or_else(|error| panic!("corruption {index} did not parse: {error:?}\n{text}"));
    assert!(
        verify_mir_bubble(&corrupted, types).is_err(),
        "corrupt immutable Bytes borrow was accepted:\n{text}"
    );
}
