#![allow(clippy::redundant_closure_for_method_calls)]

use pop_backend_mir_interp::{MirInterpreter, MirValue, ReferenceRuntimeEvent};
use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, analyze_bubble, decode_reference_metadata,
    encode_reference_metadata,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_mir::{
    MirBubble, MirCleanupExitReason, MirDeclarationKind, lower_hir_bubble, optimize_mir,
    parse_mir_dump, verify_mir_bubble,
};
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

#[test]
fn codec_error_cases_execute_with_exact_exhaustive_identities() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private function classify(error: Codec.Error): Int\n\
             match error\n\
             when Codec.Error.MalformedInput then\n\
                 return 1\n\
             when Codec.Error.LimitExceeded then\n\
                 return 2\n\
             when Codec.Error.CapabilityFailure then\n\
                 return 3\n\
             end\n\
         end\n\
         public function run(): Int\n\
             local malformed = classify(Codec.Error.MalformedInput)\n\
             local limit = classify(Codec.Error.LimitExceeded)\n\
             local capability = classify(Codec.Error.CapabilityFailure)\n\
             return malformed * 100 + limit * 10 + capability\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("123")]);
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
fn completed_async_tasks_reuse_the_exact_completion_after_optimization() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private async function build(): (Int, Int)\n\
             return 20, 22\n\
         end\n\
         public async function run(): Int\n\
             local task = build()\n\
             local first = await task\n\
             local second = await task\n\
             return first[1] + second[2]\n\
         end\n",
        "run",
    );

    let (returned, events) = execute_pair(&mir, &arena, entry);
    assert_eq!(returned, vec![integer("42")]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ReferenceRuntimeEvent::AllocateObject { .. }))
            .count(),
        2,
        "one cold task and one retained tuple completion must be allocated: {events:?}"
    );
    let task_map = events.iter().find_map(|event| match event {
        ReferenceRuntimeEvent::AllocateObject { object_map, .. }
            if object_map.slot_count() == 1 =>
        {
            Some(object_map)
        }
        _ => None,
    });
    assert!(
        task_map.is_some_and(|map| {
            map.reference_slots() == [pop_runtime_interface::ObjectSlot::new(0)]
        }),
        "the retained managed completion must occupy the compiler-proven task slot: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ReferenceRuntimeEvent::WriteBarrier(_))),
        "publishing the managed completion must use the task object's barrier: {events:?}"
    );
}

#[test]
fn cold_task_arguments_evaluate_once_before_start_after_optimization() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private async function identity(value: Int): Int\n\
             return value\n\
         end\n\
         public async function run(): Int\n\
             local counter = 0\n\
             local function next(): Int\n\
                 counter += 1\n\
                 return counter\n\
             end\n\
             local task = identity(next())\n\
             local beforeAwait = counter\n\
             local first = await task\n\
             local second = await task\n\
             return beforeAwait * 100 + first * 10 + second\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("111")]);
}

#[test]
fn indirect_async_closures_preserve_typed_captures_after_optimization() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public async function run(): Int\n\
             local base = 40\n\
             local operation = async function(value: Int): Int\n\
                 return base + value\n\
             end\n\
             return await operation(2)\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("42")]);
}

#[test]
fn specialized_generics_execute_identically_before_and_after_optimization() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private record Box<T>\n\
             value: T\n\
         end\n\
         private union Choice<T>\n\
             Value(value: T)\n\
             Empty\n\
         end\n\
         private function identity<T>(value: T): T\n\
             return value\n\
         end\n\
         private function boxed<T>(value: T): Box<T>\n\
             local result: Box<T> = { value = identity<<T>>(value) }\n\
             return result\n\
         end\n\
         private function choose<T>(value: T): Choice<T>\n\
             return Choice.Value<<T>>(value)\n\
         end\n\
         public function run(): Int\n\
             local box: Box<Int> = boxed<<Int>>(7)\n\
             local choice: Choice<Int> = choose<<Int>>(box.value)\n\
             match choice\n\
             when Choice.Value(value) then\n\
                 return value\n\
             when Choice.Empty then\n\
                 return 0\n\
             end\n\
         end\n",
        "run",
    );

    let (returned, _) = execute_pair(&mir, &arena, entry);
    assert_eq!(returned, vec![integer("7")]);
    assert!(mir.functions().iter().all(|function| {
        function
            .parameters()
            .iter()
            .chain(function.results())
            .all(|type_id| !arena.contains_type_parameter(*type_id))
    }));
}

#[test]
fn generic_nominal_iterator_executes_identically_before_and_after_optimization() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private class ArrayIterator<T> implements Iterator<T>\n\
             private values: {T}\n\
             private index: Int\n\
             public function ArrayIterator.new(values: {T}): ArrayIterator<T>\n\
                 return ArrayIterator { values = values, index = 1 }\n\
             end\n\
             public function ArrayIterator:iterator(): Iterator<T>\n\
                 return self\n\
             end\n\
             public function ArrayIterator:next(): Iteration<T>\n\
                 if self.index > Array.length(self.values) then\n\
                     return Iteration.End\n\
                 end\n\
                 local value = Array.get(self.values, self.index)\n\
                 self.index += 1\n\
                 return Iteration.Item(value)\n\
             end\n\
         end\n\
         public function run(): Int\n\
             local values: {Int} = {1, 2, 3}\n\
             local iterator: ArrayIterator<Int> = ArrayIterator.new(values)\n\
             local total = 0\n\
             for value in iterator do\n\
                 total += value\n\
             end\n\
             return total\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("6")]);
    assert!(!mir.dump().to_ascii_lowercase().contains("lookup name"));
}

#[test]
fn escaping_immutable_closure_uses_portable_environment_allocation_events() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         private function makeCounter(start: Int): function(delta: Int): Int\n\
             return function(delta: Int): Int\n\
                 return start\n\
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
    assert_eq!(returned, vec![integer("1")]);
    assert!(
        events
            .iter()
            .filter(|event| matches!(event, ReferenceRuntimeEvent::AllocateObject { .. }))
            .count()
            >= 1,
        "the escaping environment must be an explicit PLRI allocation: {events:?}"
    );
    let dump = mir.dump();
    assert!(dump.contains("closure"));
    assert!(!dump.contains("captureCell.allocate"));
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
                 return 5\n\
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

#[test]
fn checked_nominal_cast_preserves_identity_and_returns_typed_absence() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public interface Reader\n\
             function read(): Int\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(): Int\n\
                 return 1\n\
             end\n\
         end\n\
         public class SocketReader implements Reader\n\
             public function SocketReader:read(): Int\n\
                 return 2\n\
             end\n\
         end\n\
         private function isFileReader(reader: Reader): Boolean\n\
             return FileReader(reader) ~= nil\n\
         end\n\
         public function run(): Int\n\
             local fileReader = FileReader {}\n\
             local socketReader = SocketReader {}\n\
             if isFileReader(fileReader) and not isFileReader(socketReader) then\n\
                 return 42\n\
             end\n\
             return 1\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("42")]);
}

#[test]
fn checked_nominal_cast_rejects_a_different_generic_specialization() {
    let text = "namespace Main\n\
         public interface Reader<T>\n\
             function read(): T\n\
         end\n\
         public class Box<T> implements Reader<T>\n\
             public value: T\n\
             public function Box.new(value: T): Box<T>\n\
                 return Box { value = value }\n\
             end\n\
             public function Box:read(): T\n\
                 return self.value\n\
             end\n\
         end\n\
         public function makeInt(): Reader<Int>\n\
             local box: Box<Int> = Box.new(1)\n\
             return box\n\
         end\n\
         public function makeString(): Reader<String>\n\
             local box: Box<String> = Box.new(\"wrong\")\n\
             return box\n\
         end\n\
         public function isIntBox(reader: Reader<Int>): Boolean\n\
             return Box<Int>(reader) ~= nil\n\
         end\n";
    let source = SourceFile::new(FileId::from_raw(0), "src/genericCast.pop", text)
        .expect("generic cast source");
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
    let hir = front_end.hir().expect("generic cast HIR");
    let symbol = |name: &str| {
        hir.functions()
            .iter()
            .find(|function| function.name() == name)
            .expect("test function")
            .symbol()
    };
    let make_int = symbol("makeInt");
    let make_string = symbol("makeString");
    let is_int_box = symbol("isIntBox");
    let arena = front_end.types().clone();
    let mir = lower_hir_bubble(hir, front_end.types()).expect("generic cast MIR");
    let interpreter = MirInterpreter::new(&mir, &arena).expect("generic cast interpreter");
    let accepted = interpreter
        .call(make_int, &[])
        .expect("make Int specialization");
    let rejected = interpreter
        .call(make_string, &[])
        .expect("make String specialization");

    assert_eq!(
        interpreter
            .call(is_int_box, &accepted)
            .expect("matching generic cast"),
        vec![MirValue::Boolean(true)]
    );
    assert_eq!(
        interpreter
            .call(is_int_box, &rejected)
            .expect("invariant generic cast"),
        vec![MirValue::Boolean(false)]
    );
}

#[test]
fn checked_nominal_cast_uses_the_producer_bubble_identity_after_reference_import() {
    let producer_bubble = BubbleId::from_raw(41);
    let producer_source = SourceFile::new(
        FileId::from_raw(0),
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
         public function make(): Reader\n\
             local reader: FileReader = FileReader {}\n\
             return reader\n\
         end\n",
    )
    .expect("producer source");
    let producer = analyze_bubble(FrontEndBubbleInput::new(
        producer_bubble,
        NamespaceId::from_raw(41),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), producer_source)],
    ));
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );
    let producer_hir = producer
        .hir()
        .unwrap_or_else(|| panic!("producer HIR: {:?}", producer.hir_build_errors()));
    let make = producer_hir
        .functions()
        .iter()
        .find(|function| function.name() == "make")
        .expect("producer make")
        .symbol();
    let producer_mir = lower_hir_bubble(producer_hir, producer.types()).expect("producer MIR");

    let other_source = SourceFile::new(
        FileId::from_raw(2),
        "src/otherContracts.pop",
        "namespace Other.Contracts\n\
         public interface Reader\n\
             function read(): Int\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(): Int\n\
                 return 2\n\
             end\n\
         end\n\
         public function make(): Reader\n\
             local reader: FileReader = FileReader {}\n\
             return reader\n\
         end\n",
    )
    .expect("other producer source");
    let other = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(43),
        NamespaceId::from_raw(43),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(2), other_source)],
    ));
    assert!(
        other.diagnostics().is_empty(),
        "{}",
        other.diagnostic_snapshot()
    );
    let other_hir = other.hir().expect("other producer HIR");
    let other_make = other_hir
        .functions()
        .iter()
        .find(|function| function.name() == "make")
        .expect("other producer make")
        .symbol();
    let other_mir = lower_hir_bubble(other_hir, other.types()).expect("other producer MIR");

    let consumer_source = SourceFile::new(
        FileId::from_raw(1),
        "src/main.pop",
        "namespace Application\n\
         using Library.Contracts\n\
         public function isFileReader(reader: Reader): Boolean\n\
             return FileReader(reader) ~= nil\n\
         end\n",
    )
    .expect("consumer source");
    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(42),
            NamespaceId::from_raw(42),
            vec![producer_bubble],
            vec![FrontEndModule::new(ModuleId::from_raw(1), consumer_source)],
        )
        .with_reference_metadata(vec![
            producer
                .reference_metadata()
                .expect("producer reference metadata")
                .clone(),
        ]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let consumer_hir = consumer.hir().expect("consumer HIR");
    let is_file_reader = consumer_hir
        .functions()
        .iter()
        .find(|function| function.name() == "isFileReader")
        .expect("consumer cast")
        .symbol();
    let consumer_mir = lower_hir_bubble(consumer_hir, consumer.types()).expect("consumer MIR");

    let producer_interpreter =
        MirInterpreter::new(&producer_mir, producer.types()).expect("producer interpreter");
    let value = producer_interpreter
        .call(make, &[])
        .expect("producer execution");
    let other_interpreter =
        MirInterpreter::new(&other_mir, other.types()).expect("other producer interpreter");
    let wrong_bubble_value = other_interpreter
        .call(other_make, &[])
        .expect("other producer execution");
    let consumer_interpreter =
        MirInterpreter::new(&consumer_mir, consumer.types()).expect("consumer interpreter");
    assert_eq!(
        consumer_interpreter
            .call(is_file_reader, &value)
            .expect("consumer execution"),
        vec![MirValue::Boolean(true)]
    );
    assert_eq!(
        consumer_interpreter
            .call(is_file_reader, &wrong_bubble_value)
            .expect("wrong-Bubble consumer execution"),
        vec![MirValue::Boolean(false)]
    );
}

#[test]
fn checked_nominal_cast_uses_exact_specialized_artifact_ancestry() {
    let producer_bubble = BubbleId::from_raw(91);
    let source = SourceFile::new(
        FileId::from_raw(0),
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
             public function Derived.new(value: T): Derived<T>\n\
                 return Derived { value = value }\n\
             end\n\
             public function Derived:read(): T\n\
                 return self.value\n\
             end\n\
         end\n\
         public function make(): Reader<Int>\n\
             local value: Derived<Int> = Derived.new(42)\n\
             return value\n\
         end\n",
    )
    .expect("producer source");
    let producer = analyze_bubble(FrontEndBubbleInput::new(
        producer_bubble,
        NamespaceId::from_raw(91),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );
    let producer_hir = producer.hir().expect("producer HIR");
    let make = producer_hir
        .functions()
        .iter()
        .find(|function| function.name() == "make")
        .unwrap()
        .symbol();
    let producer_mir = lower_hir_bubble(producer_hir, producer.types()).expect("producer MIR");
    let metadata = producer.reference_metadata().expect("metadata");
    let base = metadata
        .classes()
        .iter()
        .find(|class| class.name() == "Base")
        .unwrap();
    let mut encoded = String::from_utf8(encode_reference_metadata(metadata).unwrap()).unwrap();
    let marker = "\"name\":\"Derived\",\"type_parameter_count\":1,\"is_open\":false,\"interface_witnesses\":";
    let replacement = format!(
        "\"name\":\"Derived\",\"type_parameter_count\":1,\"is_open\":false,\"direct_base\":{{\"definition\":{{\"bubble\":{},\"symbol\":{}}},\"arguments\":[{{\"TypeParameter\":0}}]}},\"interface_witnesses\":",
        base.identity().bubble().raw(),
        base.identity().symbol().raw()
    );
    encoded = encoded.replacen(marker, &replacement, 1);
    let metadata = decode_reference_metadata(encoded.as_bytes()).expect("typed ancestry");
    let consumer_source = SourceFile::new(FileId::from_raw(1), "src/main.pop", "namespace Application\nusing Library.Contracts\nprivate function retainType(value: Derived<Int>): Derived<Int>\nreturn value\nend\npublic function cast(reader: Reader<Int>): Boolean\nreturn Base<Int>(reader) ~= nil\nend\n").unwrap();
    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(92),
            NamespaceId::from_raw(92),
            vec![producer_bubble],
            vec![FrontEndModule::new(ModuleId::from_raw(1), consumer_source)],
        )
        .with_reference_metadata(vec![metadata]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let consumer_hir = consumer.hir().expect("consumer HIR");
    let cast = consumer_hir
        .functions()
        .iter()
        .find(|function| function.name() == "cast")
        .unwrap()
        .symbol();
    let consumer_mir = lower_hir_bubble(consumer_hir, consumer.types()).expect("consumer MIR");
    let value = MirInterpreter::new(&producer_mir, producer.types())
        .unwrap()
        .call(make, &[])
        .unwrap();
    assert_eq!(
        MirInterpreter::new(&consumer_mir, consumer.types())
            .unwrap()
            .call(cast, &value)
            .unwrap(),
        vec![MirValue::Boolean(true)],
        "producer:\n{}\nconsumer:\n{}",
        producer_mir.dump(),
        consumer_mir.dump()
    );
}

#[test]
fn checked_nominal_cast_accepts_a_verified_transitive_class_descriptor() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public interface Reader\n\
             function read(): Int\n\
         end\n\
         public open class FileReader implements Reader\n\
             public function FileReader:read(): Int\n\
                 return 1\n\
             end\n\
         end\n\
         public class BufferedReader implements Reader\n\
             public function BufferedReader:read(): Int\n\
                 return 2\n\
             end\n\
         end\n\
         private function isFileReader(reader: Reader): Boolean\n\
             return FileReader(reader) ~= nil\n\
         end\n\
         public function run(): Int\n\
             local bufferedReader = BufferedReader {}\n\
             return if isFileReader(bufferedReader) then 42 else 1\n\
         end\n",
        "run",
    );
    let classes = mir
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Class(class) => Some((declaration.symbol(), class.class())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(_, target), (descendant_symbol, descendant)] = classes.as_slice() else {
        panic!("expected target and descendant class descriptors");
    };
    let dump = mir.dump();
    let prefix = format!(
        "type.class s{} c{} ",
        descendant_symbol.raw(),
        descendant.raw()
    );
    let original = dump
        .lines()
        .find(|line| line.starts_with(&prefix))
        .expect("descendant class descriptor");
    let with_ancestry = dump.replacen(original, &format!("{original} base c{}", target.raw()), 1);
    let descendant_mir = parse_mir_dump(&with_ancestry).expect("class ancestry MIR text");
    verify_mir_bubble(&descendant_mir, &arena).expect("verified class ancestry");

    assert_eq!(
        execute_pair(&descendant_mir, &arena, entry).0,
        vec![integer("42")]
    );
}

#[test]
fn typed_failure_and_cleanup_execute_identically_before_and_after_optimization() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         --- <summary>\n\
         --- Describes loading failures.\n\
         --- </summary>\n\
         public error LoadError\n\
             --- <summary>\n\
             --- Loading failed.\n\
             --- </summary>\n\
             Failed\n\
         end\n\
         public class Marker\n\
             public count: Int = 0\n\
         end\n\
         private function fail(): Result<Int, LoadError>\n\
             return Result.Error(LoadError.Failed())\n\
         end\n\
         private function forward(marker: Marker): Result<Int, LoadError>\n\
             defer\n\
                 marker.count = marker.count + 1\n\
             end\n\
             local value = try fail()\n\
             return Result.Ok(value)\n\
         end\n\
         public function run(): Int\n\
             local marker = Marker {}\n\
             local result = forward(marker)\n\
             match result\n\
             when Result.Ok(value) then\n\
                 return value\n\
             when Result.Error(error) then\n\
                 match error\n\
                 when LoadError.Failed then\n\
                     return marker.count\n\
                 end\n\
             end\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("1")]);
}

#[test]
fn cleanup_registration_is_conditional_lifo_and_covers_fallthrough_and_loop_exits() {
    let (mir, arena, entry) = lower(
        "namespace Main\n\
         public class Marker\n\
             public count: Int = 0\n\
         end\n\
         private function exercise(marker: Marker): Int\n\
             if true then\n\
                 defer\n\
                     marker.count = marker.count * 10 + 1\n\
                 end\n\
                 defer\n\
                     marker.count = marker.count * 10 + 2\n\
                 end\n\
             end\n\
             if false then\n\
                 defer\n\
                     marker.count = marker.count * 10 + 9\n\
                 end\n\
             end\n\
             local index = 0\n\
             while index < 3 do\n\
                 index += 1\n\
                 defer\n\
                     marker.count = marker.count * 10 + 3\n\
                 end\n\
                 if index == 1 then\n\
                     continue\n\
                 end\n\
                 defer\n\
                     marker.count = marker.count * 10 + 4\n\
                 end\n\
                 break\n\
             end\n\
             return marker.count\n\
         end\n\
         public function run(): Int\n\
             return exercise(Marker {})\n\
         end\n",
        "run",
    );

    assert_eq!(execute_pair(&mir, &arena, entry).0, vec![integer("21343")]);
    let cleanup_reasons: Vec<_> = mir
        .functions()
        .iter()
        .flat_map(|function| function.blocks())
        .filter_map(|block| block.cleanup().map(|cleanup| cleanup.reason()))
        .collect();
    for reason in [
        MirCleanupExitReason::Normal,
        MirCleanupExitReason::Break,
        MirCleanupExitReason::Continue,
    ] {
        assert!(cleanup_reasons.contains(&reason), "missing {reason:?}");
    }
}
