use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{MirEffect, lower_hir_bubble, parse_mir_dump, verify_mir_bubble};
use pop_source::SourceFile;

fn lower(source: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", source).expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    (mir, front_end.types().clone())
}

fn assert_verified_round_trip(mir: &pop_mir::MirBubble, types: &pop_types::TypeArena) {
    assert!(verify_mir_bubble(mir, types).is_ok());
    let dump = mir.dump();
    let reparsed = parse_mir_dump(&dump).expect("MIR dump parses");
    assert_eq!(reparsed.dump(), dump);
    assert!(verify_mir_bubble(&reparsed, types).is_ok());
}

#[test]
fn closure_conversion_uses_typed_cells_environments_maps_and_safe_points() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function count(start: Int): Int\n\
             local current = start\n\
             local function advance(): Int\n\
                 current = current + 1\n\
                 return current\n\
             end\n\
             return advance()\n\
         end\n",
    );

    let dump = mir.dump();
    assert!(dump.contains("captureCell.allocate"), "{dump}");
    assert!(dump.contains("closureEnvironment.allocate"), "{dump}");
    assert!(dump.contains("capture.load"), "{dump}");
    assert!(dump.contains("capture.store"), "{dump}");
    assert!(dump.contains("callIndirect"), "{dump}");
    assert!(dump.contains("map[1:0]"), "{dump}");
    assert!(dump.contains("gcSafePoint"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("table"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("lookup name"), "{dump}");
    assert_verified_round_trip(&mir, &types);
}

#[test]
fn exhaustive_union_match_is_one_resolved_switch_with_typed_payload_blocks() {
    let (mir, types) = lower(
        "namespace Main\n\
         public union Choice\n\
             Some(value: Int)\n\
             None\n\
         end\n\
         public function choose(choice: Choice): Int\n\
             match choice\n\
             when Choice.Some(value) then\n\
                 return value\n\
             when Choice.None then\n\
                 return 0\n\
             end\n\
         end\n",
    );

    let integer = types.source_type("Int").expect("Int");
    let dump = mir.dump();
    assert_eq!(dump.matches("union.switch").count(), 1, "{dump}");
    assert!(dump.contains("case#0"), "{dump}");
    assert!(dump.contains("case#1"), "{dump}");
    assert!(dump.contains(&format!(":t{}", integer.raw())), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("tag name"), "{dump}");
    assert_verified_round_trip(&mir, &types);
}

#[test]
fn numeric_conversions_and_complete_ordering_are_explicit_and_round_trip() {
    // ADR 0040: conversions preserve exact source/target kinds in canonical
    // MIR, and <=/>= are not reconstructed by a backend.
    let (mir, types) = lower(
        "namespace Main\n\
         public function convert(value: Int): Float32\n\
             return Float32(value)\n\
         end\n\
         public function narrow(value: Float64): Int16\n\
             return Int16(value)\n\
         end\n\
         public function ordered(left: Float64, right: Float64): Boolean\n\
             return left <= right and left >= right\n\
         end\n",
    );

    let dump = mir.dump();
    assert!(
        dump.contains("numeric.integerToFloat Int64 Float32"),
        "{dump}"
    );
    assert!(
        dump.contains("numeric.floatToInteger Float64 Int16"),
        "{dump}"
    );
    assert!(dump.contains("float.compareLessOrEqual Float64"), "{dump}");
    assert!(
        dump.contains("float.compareGreaterOrEqual Float64"),
        "{dump}"
    );
    assert!(!dump.to_ascii_lowercase().contains("dynamic"), "{dump}");
    assert!(!mir.functions()[0].effects().contains(MirEffect::MayTrap));
    assert!(mir.functions()[1].effects().contains(MirEffect::MayTrap));
    let narrowing = mir.functions()[1].blocks()[0].instructions()[0].kind();
    assert_eq!(
        narrowing.possible_traps(),
        vec![pop_runtime_interface::TrapKind::NumericConversion]
    );
    assert_verified_round_trip(&mir, &types);
}

#[test]
fn numeric_for_and_loop_control_lower_to_verified_portable_cfg() {
    // ADR 0042 forbids backend-specific range and loop-control instructions.
    let (mir, types) = lower(
        "namespace Main\n\
         public function sum(limit: Int): Int\n\
             local total = 0\n\
             for index = 1, limit do\n\
                 if index == 2 then\n\
                     continue\n\
                 end\n\
                 total = total + index\n\
                 if total > 10 then\n\
                     break\n\
                 end\n\
             end\n\
             return total\n\
         end\n",
    );

    let dump = mir.dump();
    assert!(dump.contains("integer.compareLessOrEqual Int64"), "{dump}");
    assert!(
        dump.contains("integer.compareGreaterOrEqual Int64"),
        "{dump}"
    );
    assert!(dump.contains("integer.checkedAdd Int64"), "{dump}");
    assert!(dump.contains("gcSafePoint"), "{dump}");
    assert!(!dump.contains("numericFor"), "{dump}");
    assert!(!dump.contains("break"), "{dump}");
    assert!(!dump.contains("continue"), "{dump}");
    assert_verified_round_trip(&mir, &types);
}

#[test]
fn typed_string_composition_is_backend_neutral_effectful_and_round_trips() {
    // ADR 0041: concatenation and primitive formatting carry exact static
    // kinds, allocation effects, and no runtime format-string lookup.
    let (mir, types) = lower(
        "namespace Main\n\
         public function describe(count: Int, ratio: Float32, enabled: Boolean): String\n\
             return `count={count}, ratio={ratio}, enabled={enabled}` .. \"!\"\n\
         end\n",
    );

    let dump = mir.dump();
    assert!(dump.contains("string.format Integer(Int64)"), "{dump}");
    assert!(dump.contains("string.format Float(Float32)"), "{dump}");
    assert!(dump.contains("string.format Boolean"), "{dump}");
    assert!(dump.contains("string.concat"), "{dump}");
    assert!(!dump.contains("pop_rt_"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("lookup"), "{dump}");
    assert!(mir.functions()[0].effects().contains(MirEffect::Allocates));
    assert!(
        mir.functions()[0]
            .effects()
            .contains(MirEffect::GcSafePoint)
    );
    assert_verified_round_trip(&mir, &types);
}

#[test]
fn nominal_interface_schema_upcast_and_slot_call_are_portable_and_round_trip() {
    let (mir, types) = lower(
        "namespace Main\n\
         private interface Closeable\n\
             function close()\n\
         end\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
         end\n\
         public function readOne(reader: FileReader): String\n\
             local contract: Reader = reader\n\
             return contract:read(1)\n\
         end\n",
    );

    let dump = mir.dump();
    assert!(dump.contains("type.interface"), "{dump}");
    assert!(dump.contains("implements"), "{dump}");
    assert!(dump.contains("interface.upcast"), "{dump}");
    assert!(dump.contains("call.interface"), "{dump}");
    assert!(dump.contains("slot#0"), "{dump}");
    assert!(!dump.contains("slot#1"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("lookup name"), "{dump}");
    assert_verified_round_trip(&mir, &types);
}
