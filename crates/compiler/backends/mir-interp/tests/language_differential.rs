use pop_backend_mir_interp::{MirInterpreter, MirValue, ReferenceRuntimeEvent};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_mir::{MirBubble, lower_hir_bubble, optimize_mir};
use pop_runtime_interface::{RuntimeFailure, Trap, TrapKind};
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
    assert!(dump.contains("captureCell.allocate"));
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
fn string_composition_and_primitive_formatting_are_optimization_stable() {
    // ADR 0041: the interpreter executes the same deterministic bytes before
    // and after MIR optimization, including UTF-8 and negative zero.
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public function run(): String\n\
             local count: Int8 = -12\n\
             local ratio: Float32 = 1.5\n\
             local negativeZero: Float64 = -0.0\n\
             return `Pop 🫧 {count} {ratio} {negativeZero} {true}` .. \"!\"\n\
         end\n",
        "run",
    );

    let expected = vec![MirValue::String("Pop 🫧 -12 1.5 -0 true!".to_owned())];
    let construction = MirInterpreter::new(&mir, &arena)
        .expect("construction interpreter")
        .call(entry, &[])
        .expect("construction execution");
    let optimized = optimize_mir(mir.clone(), &arena).expect("optimized MIR");
    let optimized = MirInterpreter::new(&optimized, &arena)
        .expect("optimized interpreter")
        .call(entry, &[])
        .expect("optimized execution");
    assert_eq!(construction, expected);
    assert_eq!(optimized, expected);
}

#[test]
fn conditional_expressions_are_lazy_and_elseif_is_optimization_stable() {
    // ADR 0043: an unselected expression branch is never evaluated, and
    // statement elseif chains preserve their source ordering.
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private function fail(): Int\n\
             return 1 / 0\n\
         end\n\
         public function run(): Int\n\
             local first = if true then 40 else fail()\n\
             local second = if false then fail() else 1\n\
             if false then\n\
                 return fail()\n\
             elseif first == 40 then\n\
                 return first + second + 1\n\
             else\n\
                 return fail()\n\
             end\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("42")]);
}

#[test]
fn compound_assignment_evaluates_targets_and_rhs_once_in_source_order() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public class State\n\
             public log: Int = 0\n\
         end\n\
         public class Box\n\
             public value: Int = 10\n\
         end\n\
         private function fieldRight(state: State, box: Box): Int\n\
             state.log = state.log * 10 + 2\n\
             box.value = 20\n\
             return 5\n\
         end\n\
         private function selectArray(state: State, values: {Int}): {Int}\n\
             state.log = state.log * 10 + 3\n\
             return values\n\
         end\n\
         private function selectIndex(state: State): Int\n\
             state.log = state.log * 10 + 4\n\
             return 1\n\
         end\n\
         private function arrayRight(state: State): Int\n\
             state.log = state.log * 10 + 5\n\
             return 4\n\
         end\n\
         public function run(): Int\n\
             local state = State {}\n\
             local box = Box {}\n\
             local values: {Int} = { 2 }\n\
             box.value += fieldRight(state, box)\n\
             selectArray(state, values)[selectIndex(state)] *= arrayRight(state)\n\
             return state.log + box.value + Array.get(values, 1)\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("2368")]);
}

#[test]
fn compound_assignment_updates_shared_capture_cells() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public function run(): Int\n\
             local value = 1\n\
             local bump = function(): Int\n\
                 value += 2\n\
                 return value\n\
             end\n\
             return bump()\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("3")]);
    assert!(mir.dump().contains("capture.store"));
}

#[test]
fn compound_array_bounds_trap_precedes_rhs_evaluation() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private function fail(): Int\n\
             return 1 / 0\n\
         end\n\
         public function run(): Int\n\
             local values: {Int} = { 1 }\n\
             values[2] += fail()\n\
             return 0\n\
         end\n",
        "run",
    );
    for candidate in [
        mir.clone(),
        optimize_mir(mir, &arena).expect("optimized MIR"),
    ] {
        let interpreter = MirInterpreter::new(&candidate, &arena).expect("interpreter");
        assert_eq!(
            interpreter.call(entry, &[]),
            Err(pop_backend_mir_interp::ExecutionError::Runtime(
                RuntimeFailure::Trap(Trap::new(TrapKind::BoundsViolation))
            ))
        );
    }
}

#[test]
fn numeric_ranges_break_and_continue_are_cfg_stable() {
    // ADR 0042: every loop-control form lowers to verified CFG edges, including
    // continue-to-condition for repeat-until.
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public function run(): Int\n\
             local total = 0\n\
             for index = 1, 6 do\n\
                 if index == 2 then\n\
                     continue\n\
                 end\n\
                 if index == 5 then\n\
                     break\n\
                 end\n\
                 total = total + index\n\
             end\n\
             for reverse = 3, 1, -1 do\n\
                 total = total + reverse\n\
             end\n\
             local current = 0\n\
             while current < 4 do\n\
                 current = current + 1\n\
                 if current == 2 then\n\
                     continue\n\
                 end\n\
                 total = total + current\n\
             end\n\
             repeat\n\
                 current = current - 1\n\
                 if current == 2 then\n\
                     continue\n\
                 end\n\
                 total = total + current\n\
             until current == 0\n\
             return total\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("26")]);
    let dump = mir.dump();
    assert!(dump.contains("integer.checkedAdd Int64"), "{dump}");
    assert!(dump.contains("integer.compareLessOrEqual Int64"), "{dump}");
    assert!(
        dump.contains("integer.compareGreaterOrEqual Int64"),
        "{dump}"
    );
    assert!(!dump.to_ascii_lowercase().contains("iterator lookup"));
}

#[test]
fn dynamic_zero_numeric_range_step_raises_the_typed_trap() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public function run(step: Int): Int\n\
             local total = 0\n\
             for index = 1, 3, step do\n\
                 total = total + index\n\
             end\n\
             return total\n\
         end\n",
        "run",
    );
    let interpreter = MirInterpreter::new(&mir, &arena).expect("interpreter");
    assert_eq!(
        interpreter.call(entry, &[integer("0")]),
        Err(pop_backend_mir_interp::ExecutionError::Runtime(
            RuntimeFailure::Trap(Trap::new(TrapKind::InvalidRangeStep))
        ))
    );
}

#[test]
fn numeric_range_inputs_evaluate_once_and_nested_control_targets_innermost_loop() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public class Marker\n\
             public value: Int = 0\n\
         end\n\
         public function run(): Int\n\
             local marker = Marker {}\n\
             local first = function(): Int\n\
                 marker.value = marker.value * 10 + 1\n\
                 return 1\n\
             end\n\
             local last = function(): Int\n\
                 marker.value = marker.value * 10 + 2\n\
                 return 2\n\
             end\n\
             local step = function(): Int\n\
                 marker.value = marker.value * 10 + 3\n\
                 return 1\n\
             end\n\
             for ignored = first(), last(), step() do\n\
                 ignored\n\
             end\n\
             local visits = 0\n\
             for outer = 1, 2 do\n\
                 for inner = 1, 3 do\n\
                     if inner == 2 then\n\
                         break\n\
                     end\n\
                     visits = visits + 1\n\
                 end\n\
                 outer\n\
             end\n\
             for empty = 5, 1 do\n\
                 visits = visits + empty\n\
             end\n\
             for empty = 1, 5, -1 do\n\
                 visits = visits + empty\n\
             end\n\
             return marker.value * 10 + visits\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("1232")]);
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
