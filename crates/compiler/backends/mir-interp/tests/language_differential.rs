use pop_backend_mir_interp::{MirInterpreter, MirValue, ReferenceRuntimeEvent};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_mir::{MirBubble, lower_hir_bubble, optimize_mir};
use pop_source::SourceFile;
use pop_types::{IntegerKind, IntegerValue, TypeArena};

fn lower(text: &str, entry: &str) -> (MirBubble, TypeArena, SymbolId) {
    let source =
        SourceFile::new(FileId::from_raw(0), "src/differential.pop", text).expect("source fixture");
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
    let hir = front_end.hir().expect("verified HIR");
    let entry = hir
        .functions()
        .iter()
        .find(|function| function.name() == entry)
        .expect("entry function")
        .symbol();
    let mir = lower_hir_bubble(hir, front_end.types()).expect("verified MIR");
    (mir, front_end.types().clone(), entry)
}

fn integer(value: &str) -> MirValue {
    MirValue::Integer(IntegerValue::parse_decimal(value, IntegerKind::Int64).expect("Int literal"))
}

fn execute_pair(
    mir: &MirBubble,
    arena: &TypeArena,
    entry: SymbolId,
) -> (Vec<MirValue>, Vec<ReferenceRuntimeEvent>) {
    let optimized = optimize_mir(mir.clone(), arena).expect("optimized MIR");
    let construction = MirInterpreter::new(mir, arena).expect("construction MIR");
    let construction_value = construction
        .call(entry, &[])
        .expect("construction execution");
    let construction_events = construction.runtime().events().to_vec();
    let optimized_interpreter =
        MirInterpreter::new(&optimized, arena).expect("optimized interpreter");
    let optimized_value = optimized_interpreter
        .call(entry, &[])
        .expect("optimized execution");
    let optimized_events = optimized_interpreter.runtime().events().to_vec();
    assert_eq!(optimized_value, construction_value);
    assert_eq!(optimized_events, construction_events);
    (construction_value, construction_events)
}

#[test]
fn escaping_mutating_closure_uses_shared_cells_and_portable_allocation_events() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private function makeCounter(start: Int): function(delta: Int): Int\n\
             local total = start\n\
             return function(delta: Int): Int\n\
                 total = total + delta\n\
                 return total\n\
             end\n\
         end\n\
         public function run(): Int\n\
             local counter = makeCounter(1)\n\
             counter(2)\n\
             return counter(3)\n\
         end\n",
        "run",
    );

    let (returned, events) = execute_pair(&mir, &arena, entry);
    assert_eq!(returned, vec![integer("6")]);
    assert!(
        events
            .iter()
            .filter(|event| matches!(event, ReferenceRuntimeEvent::AllocateObject { .. }))
            .count()
            >= 2,
        "cell and escaping environment must be explicit PLRI allocations: {events:?}"
    );
    let dump = mir.dump();
    assert!(dump.contains("closure"));
    assert!(dump.contains("capture.cell"));
    assert!(!dump.to_ascii_lowercase().contains("lookup name"));
}

#[test]
fn recursive_local_function_dispatch_is_identity_based_and_optimization_stable() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public function run(): Int\n\
             local function factorial(value: Int): Int\n\
                 if value == 0 then\n\
                     return 1\n\
                 end\n\
                 return value * factorial(value - 1)\n\
             end\n\
             return factorial(5)\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("120")]);
    assert!(!mir.dump().to_ascii_lowercase().contains("lookup name"));
}

#[test]
fn exhaustive_union_match_switches_by_case_identity_in_both_mir_forms() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public union ResultValue\n\
             Ok(value: Int)\n\
             Error(message: String)\n\
         end\n\
         private function consume(result: ResultValue): Int\n\
             match result\n\
             when ResultValue.Ok(value) then\n\
                 return value\n\
             when ResultValue.Error(_) then\n\
                 return 0\n\
             end\n\
         end\n\
         public function run(): Int\n\
             return consume(ResultValue.Ok(7))\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("7")]);
    let dump = mir.dump();
    assert!(dump.contains("union.switch"));
    assert!(!dump.to_ascii_lowercase().contains("case name"));
}

#[test]
fn nominal_interface_upcast_and_call_use_verified_slots_in_both_mir_forms() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public interface Reader\n\
             function read(value: Int): Int\n\
         end\n\
         public class IncrementReader implements Reader\n\
             public function IncrementReader:read(value: Int): Int\n\
                 return value + 1\n\
             end\n\
         end\n\
         private function readThroughInterface(reader: Reader): Int\n\
             return reader:read(4)\n\
         end\n\
         public function run(): Int\n\
             local concrete: IncrementReader = IncrementReader {}\n\
             return readThroughInterface(concrete)\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("5")]);
    let dump = mir.dump();
    assert!(dump.contains("interface.upcast"));
    assert!(dump.contains("call.interface"));
    assert!(!dump.to_ascii_lowercase().contains("lookup name"));
}
