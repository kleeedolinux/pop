use pop_backend_mir_interp::{ExecutionError, MirInterpreter, MirValue};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{
    BubbleId, EnumCaseId, FieldId, FileId, ModuleId, NamespaceId, SymbolId, UnionCaseId,
};
use pop_mir::{lower_hir_bubble, optimize_mir};
use pop_runtime_interface::{RuntimeFailure, Trap, TrapKind};
use pop_source::SourceFile;
use pop_types::{FloatKind, FloatValue, IntegerKind, IntegerValue};

fn executable_source(text: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", text).expect("source");
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
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    (mir, front_end.types().clone())
}

fn trap(kind: TrapKind) -> ExecutionError {
    ExecutionError::Runtime(RuntimeFailure::Trap(Trap::new(kind)))
}

#[test]
fn direct_calls_checked_arithmetic_and_both_cfg_branches_execute() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         private function add(left: Int, right: Int): Int\n\
             return left + right\n\
         end\n\
         public function choose(left: Int, right: Int): Int\n\
             if left < right then\n\
                 return add(left, right)\n\
             else\n\
                 return right\n\
             end\n\
         end\n",
    );
    let choose = mir
        .functions()
        .iter()
        .find(|function| function.symbol().raw() == 1)
        .expect("choose")
        .symbol();
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");

    assert_eq!(
        interpreter
            .call(choose, &[int(2), int(3)])
            .expect("then branch"),
        vec![int(5)]
    );
    assert_eq!(
        interpreter
            .call(choose, &[int(5), int(3)])
            .expect("else branch"),
        vec![int(3)]
    );
}

#[test]
fn nominal_enum_cases_preserve_identity_and_equality() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public enum Color\n\
             Red\n\
             Blue\n\
         end\n\
         public function choose(flag: Boolean): Color\n\
             return if flag then Color.Red else Color.Blue\n\
         end\n\
         public function isRed(color: Color): Boolean\n\
             return color == Color.Red\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");
    let red = MirValue::Enum {
        definition: SymbolId::from_raw(0),
        case: EnumCaseId::from_raw(0),
        discriminant: 0,
    };
    let blue = MirValue::Enum {
        definition: SymbolId::from_raw(0),
        case: EnumCaseId::from_raw(1),
        discriminant: 1,
    };

    assert_eq!(
        interpreter
            .call(mir.functions()[0].symbol(), &[MirValue::Boolean(true)])
            .expect("red"),
        vec![red.clone()]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[0].symbol(), &[MirValue::Boolean(false)])
            .expect("blue"),
        vec![blue.clone()]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[1].symbol(), &[red])
            .expect("red equality"),
        vec![MirValue::Boolean(true)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[1].symbol(), &[blue])
            .expect("blue inequality"),
        vec![MirValue::Boolean(false)]
    );
}

#[test]
fn fixed_packs_destructure_swap_and_preserve_target_before_value_order() {
    // ADR 0045: all target locations are evaluated once before RHS values,
    // then tuple projections are stored from left to right.
    let (mir, types) = executable_source(
        "namespace Main\n\
         public class Box\n\
             public value: Int = 1\n\
         end\n\
         private function split(value: Int): (Int, Int)\n\
             return value, value + 1\n\
         end\n\
         public function calculate(value: Int): Int\n\
             local left, right = split(value)\n\
             local result = split(value)\n\
             local projected = result[2]\n\
             left, right = right, left\n\
             local counter = 0\n\
             local function advance(): Int\n\
                 counter += 1\n\
                 return counter\n\
             end\n\
             local function observed(): Int\n\
                 return counter\n\
             end\n\
             local values: {Int} = { 10, 20 }\n\
             local box = Box {}\n\
             box.value, values[advance()], values[advance()] = 7, observed(), 99\n\
             return box.value * 100000 + projected * 10000 + right * 1000 + Array.get(values, 1) * 100 + Array.get(values, 2)\n\
         end\n",
    );
    let calculate = mir.functions().last().expect("calculate").symbol();
    let expected = vec![int(754_299)];
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");
    assert_eq!(
        interpreter.call(calculate, &[int(4)]).expect("fixed pack"),
        expected
    );

    let optimized = optimize_mir(mir.clone(), &types).expect("optimized fixed-pack MIR");
    let optimized_interpreter =
        MirInterpreter::new(&optimized, &types).expect("verified optimized MIR");
    assert_eq!(
        optimized_interpreter
            .call(calculate, &[int(4)])
            .expect("optimized fixed pack"),
        expected
    );
}

#[test]
fn typed_tables_lookup_replace_insert_and_preserve_insertion_order() {
    // ADR 0046: replacement keeps position and insertion appends.
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function build(): {[String]: Int}\n\
             local scores: {[String]: Int} = { alice = 10 }\n\
             scores[\"alice\"] = 11\n\
             scores[\"bruno\"] = 12\n\
             return scores\n\
         end\n\
         public function lookup(key: String): Int?\n\
             local scores: {[String]: Int} = { alice = 10 }\n\
             scores[\"bruno\"] = 12\n\
             return scores[key]\n\
         end\n",
    );
    let build = mir.functions()[0].symbol();
    let lookup = mir.functions()[1].symbol();
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");
    assert_eq!(
        interpreter.call(build, &[]).expect("table build"),
        vec![MirValue::Table(vec![
            (MirValue::String("alice".to_owned()), int(11)),
            (MirValue::String("bruno".to_owned()), int(12)),
        ])]
    );
    assert_eq!(
        interpreter
            .call(lookup, &[MirValue::String("bruno".to_owned())])
            .expect("present key"),
        vec![int(12)]
    );
    assert_eq!(
        interpreter
            .call(lookup, &[MirValue::String("missing".to_owned())])
            .expect("missing key"),
        vec![MirValue::Nil]
    );

    let optimized = optimize_mir(mir, &types).expect("optimized table MIR");
    let optimized_interpreter =
        MirInterpreter::new(&optimized, &types).expect("verified optimized MIR");
    assert_eq!(
        optimized_interpreter
            .call(lookup, &[MirValue::String("bruno".to_owned())])
            .expect("optimized present key"),
        vec![int(12)]
    );
}

#[test]
fn mutable_locals_flow_through_loop_backedges_and_branch_joins() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function calculate(doubleValue: Boolean): Int\n\
             local value = 0\n\
             while value < 10 do\n\
                 value = value + 1\n\
             end\n\
             if doubleValue then\n\
                 value = value + value\n\
             else\n\
                 value = value + 1\n\
             end\n\
             return value\n\
         end\n",
    );
    let function = mir.functions()[0].symbol();
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");

    assert_eq!(
        interpreter
            .call(function, &[MirValue::Boolean(true)])
            .expect("then branch"),
        vec![int(20)]
    );
    assert_eq!(
        interpreter
            .call(function, &[MirValue::Boolean(false)])
            .expect("else branch"),
        vec![int(11)]
    );
}

#[test]
fn repeat_until_executes_once_and_repeats_through_its_false_backedge() {
    // ADR 0032: the body runs before the first condition check, and `false`
    // returns to the body while `true` exits.
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function countToThree(): Int\n\
             local value = 0\n\
             repeat\n\
                 local nextValue = value + 1\n\
                 value = nextValue\n\
             until nextValue == 3\n\
             return value\n\
         end\n\
         public function runOnce(): Int\n\
             local value = 0\n\
             repeat\n\
                 value = value + 1\n\
             until true\n\
             return value\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified repeat-until MIR");

    assert_eq!(
        interpreter
            .call(mir.functions()[0].symbol(), &[])
            .expect("repeat backedge execution"),
        vec![int(3)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[1].symbol(), &[])
            .expect("at-least-once execution"),
        vec![int(1)]
    );
}

#[test]
fn standard_print_overloads_execute_by_trusted_identity_and_return_no_value() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function run(): Int\n\
             print(42)\n\
             print(\"teste\")\n\
             print(\"\")\n\
             print(\"Pop 🫧\")\n\
             return 0\n\
         end\n",
    );
    assert!(mir.dump().contains("callStandard sf0"));
    assert!(mir.dump().contains("callStandard sf1"));
    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(mir.functions()[0].symbol(), &[])
            .expect("standard print call"),
        vec![int(0)]
    );
}

#[test]
fn declared_functions_flow_through_typed_values_and_indirect_calls() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         private function increment(value: Int): Int\n\
             return value + 1\n\
         end\n\
         private function apply(operation: function(value: Int): Int, value: Int): Int\n\
             return operation(value)\n\
         end\n\
         public function run(value: Int): Int\n\
             local operation: function(value: Int): Int = increment\n\
             return apply(operation, value)\n\
         end\n",
    );
    let run = mir.functions()[2].symbol();

    assert!(mir.dump().contains("callIndirect"));
    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(run, &[int(41)])
            .expect("indirect call"),
        vec![int(42)]
    );
}

#[test]
fn integer_overflow_and_division_by_zero_are_deterministic_traps() {
    for (operator, expected) in [
        ("+", trap(TrapKind::IntegerOverflow)),
        ("/", trap(TrapKind::DivisionByZero)),
    ] {
        let source = format!(
            "namespace Main\n\
             public function calculate(left: Int, right: Int): Int\n\
                 return left {operator} right\n\
             end\n"
        );
        let (mir, types) = executable_source(&source);
        let function = mir.functions()[0].symbol();
        let arguments = if operator == "+" {
            [int(i64::MAX), int(1)]
        } else {
            [int(1), int(0)]
        };
        let error = MirInterpreter::new(&mir, &types)
            .expect("verified")
            .call(function, &arguments)
            .expect_err("trap");
        assert_eq!(error, expected);
    }
}

#[test]
fn tuples_records_unions_and_false_loops_share_one_mir_runtime_model() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public record Score\n\
             value: Int\n\
         end\n\
         public union State\n\
             Idle\n\
             Ready(score: Score)\n\
         end\n\
         public function increment(score: Score): Score\n\
             while false do\n\
                 score.value\n\
             end\n\
             return score with { value = score.value + 1, }\n\
         end\n\
         public function pair(): (Int, String)\n\
             return (7, \"ready\")\n\
         end\n\
         public function idle(): State\n\
             return State.Idle\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");
    let increment = mir.functions()[0].symbol();
    let pair = mir.functions()[1].symbol();
    let idle = mir.functions()[2].symbol();

    assert_eq!(
        interpreter
            .call(
                increment,
                &[MirValue::Record {
                    record: SymbolId::from_raw(0),
                    fields: vec![(FieldId::from_raw(0), int(4))],
                }],
            )
            .expect("record update"),
        vec![MirValue::Record {
            record: SymbolId::from_raw(0),
            fields: vec![(FieldId::from_raw(0), int(5))],
        }]
    );
    assert_eq!(
        interpreter.call(pair, &[]).expect("tuple"),
        vec![MirValue::Tuple(vec![
            int(7),
            MirValue::String("ready".to_owned()),
        ])]
    );
    assert_eq!(
        interpreter.call(idle, &[]).expect("union"),
        vec![MirValue::Union {
            union: SymbolId::from_raw(1),
            case: UnionCaseId::from_raw(0),
            arguments: Vec::new(),
        }]
    );
}

#[test]
fn omitted_record_defaults_execute_as_complete_typed_values() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public record Options\n\
             name: String\n\
             attempts: Int = 3\n\
             enabled: Boolean = true\n\
         end\n\
         public function defaults(): (Int, Boolean)\n\
             local options: Options = { name = \"pop\", }\n\
             return (options.attempts, options.enabled)\n\
         end\n",
    );
    let function = mir.functions()[0].symbol();

    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(function, &[])
            .expect("record defaults"),
        vec![MirValue::Tuple(vec![int(3), MirValue::Boolean(true),])]
    );
}

#[test]
fn structural_records_keep_named_defaults_and_ignore_initializer_field_order_in_equality() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public record First\n\
             value: Int = 1\n\
         end\n\
         public record Second\n\
             value: Int = 2\n\
         end\n\
         public record Pair\n\
             left: Int\n\
             right: Int\n\
         end\n\
         public function first(): Int\n\
             local value: First = {}\n\
             return value.value\n\
         end\n\
         public function second(): Int\n\
             local value: Second = {}\n\
             return value.value\n\
         end\n\
         public function equalInAnyOrder(): Boolean\n\
             local first: Pair = { left = 1, right = 2, }\n\
             local second: Pair = { right = 2, left = 1, }\n\
             return first == second\n\
         end\n\
         private function secondArgument(value: Second): Int\n\
             return value.value\n\
         end\n\
         public function callSecondWithDefault(): Int\n\
             return secondArgument({})\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");

    assert_eq!(
        interpreter
            .call(mir.functions()[0].symbol(), &[])
            .expect("First default"),
        vec![int(1)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[1].symbol(), &[])
            .expect("Second default"),
        vec![int(2)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[2].symbol(), &[])
            .expect("structural equality"),
        vec![MirValue::Boolean(true)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[4].symbol(), &[])
            .expect("named parameter default"),
        vec![int(2)]
    );
}

#[test]
fn arrays_and_tables_execute_identically_before_and_after_mir_optimization() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function collections(): ({String}, {[String]: Int})\n\
             local names: {String} = { \"first\", \"second\" }\n\
             local scores: {[String]: Int} = { first = 1, second = 2 }\n\
             names[2] = \"updated\"\n\
             return (names, scores)\n\
         end\n",
    );
    let function = mir.functions()[0].symbol();
    let optimized = optimize_mir(mir.clone(), &types).expect("optimized MIR");
    let expected = vec![MirValue::Tuple(vec![
        MirValue::Array(vec![
            MirValue::String("first".to_owned()),
            MirValue::String("updated".to_owned()),
        ]),
        MirValue::Table(vec![
            (MirValue::String("first".to_owned()), int(1)),
            (MirValue::String("second".to_owned()), int(2)),
        ]),
    ])];

    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(function, &[])
            .expect("collections"),
        expected
    );
    assert_eq!(
        MirInterpreter::new(&optimized, &types)
            .expect("verified optimized MIR")
            .call(function, &[])
            .expect("optimized collections"),
        expected
    );
}

#[test]
fn array_indexing_is_one_based_and_returns_nil_out_of_bounds() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function at(values: {String}, index: Int): String?\n\
             return values[index]\n\
         end\n",
    );
    let function = mir.functions()[0].symbol();
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");
    let values = MirValue::Array(vec![
        MirValue::String("first".to_owned()),
        MirValue::String("second".to_owned()),
    ]);

    assert_eq!(
        interpreter
            .call(function, &[values.clone(), int(1)])
            .expect("first element"),
        vec![MirValue::String("first".to_owned())]
    );
    assert_eq!(
        interpreter
            .call(function, &[values.clone(), int(0)])
            .expect("zero index"),
        vec![MirValue::Nil]
    );
    assert_eq!(
        interpreter
            .call(function, &[values, int(3)])
            .expect("past the end"),
        vec![MirValue::Nil]
    );
}

#[test]
fn fixed_array_core_operations_execute_with_one_based_checked_semantics() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function arrays(): (Int, Int, Int?)\n\
             local values = Array.create<<Int>>(4, 0)\n\
             Array.fill(values, 7)\n\
             values[1] = 3\n\
             return (Array.length(values), Array.get(values, 1), values[5])\n\
         end\n",
    );
    let function = mir.functions()[0].symbol();
    let expected = vec![MirValue::Tuple(vec![int(4), int(3), MirValue::Nil])];

    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(function, &[])
            .expect("array core operations"),
        expected
    );
    let optimized = optimize_mir(mir, &types).expect("optimized MIR");
    assert_eq!(
        MirInterpreter::new(&optimized, &types)
            .expect("verified optimized MIR")
            .call(function, &[])
            .expect("optimized array core operations"),
        expected
    );
}

#[test]
fn fixed_array_negative_lengths_and_checked_bounds_trap() {
    for source in [
        "namespace Main\npublic function fail(): Int\nlocal values = Array.create<<Int>>(-1, 0)\nreturn 0\nend\n",
        "namespace Main\npublic function fail(): Int\nlocal values = Array.create<<Int>>(1, 0)\nreturn Array.get(values, 2)\nend\n",
    ] {
        let (mir, types) = executable_source(source);
        let function = mir.functions()[0].symbol();
        assert!(matches!(
            MirInterpreter::new(&mir, &types)
                .expect("verified MIR")
                .call(function, &[]),
            Err(ExecutionError::Runtime(RuntimeFailure::Trap(trap)))
                if trap.kind() == TrapKind::BoundsViolation
        ));
    }
}

#[test]
fn native_class_construction_and_resolved_fields_execute_without_tables() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public class Counter\n\
             public value: Int\n\
             public step: Int = 2\n\
             public function Counter.new(value: Int): Counter\n\
                 return Counter { value = value }\n\
             end\n\
             public function Counter:add(delta: Int): Counter\n\
                 self.value = self.value + delta\n\
                 return self\n\
             end\n\
             public function Counter:get(): Int\n\
                 return self.value + self.step\n\
             end\n\
         end\n\
         public function read(value: Int): Int\n\
             local counter = Counter.new(value)\n\
             counter:add(3)\n\
             return counter:get()\n\
         end\n",
    );
    let function = mir.functions()[0].symbol();

    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(function, &[int(7)])
            .expect("class construction"),
        vec![int(12)]
    );
}

#[test]
fn equality_preserves_value_and_native_class_identity_semantics() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public record Point\n\
             x: Int\n\
             name: String\n\
         end\n\
         public class Token\n\
             public value: Int\n\
         end\n\
         public function compare(value: Int): (Boolean, Boolean, Boolean, Boolean, Boolean, Boolean)\n\
             local left: Point = { x = value, name = \"pop\" }\n\
             local right: Point = { x = value, name = \"pop\" }\n\
             local first = Token { value = value }\n\
             local alias = first\n\
             local other = Token { value = value }\n\
             return (value == 7, \"pop\" ~= \"lua\", left == right, (1, \"x\") == (1, \"x\"), first == alias, first ~= other)\n\
         end\n",
    );
    let function = mir.functions()[0].symbol();

    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(function, &[int(7)])
            .expect("equality"),
        vec![MirValue::Tuple(vec![MirValue::Boolean(true); 6])]
    );
}

#[test]
fn logical_operators_short_circuit_before_trapping_right_operands() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         private function trap(): Boolean\n\
             return 1 / 0 > 0\n\
         end\n\
         public function falseAnd(): Boolean\n\
             return false and trap()\n\
         end\n\
         public function trueOr(): Boolean\n\
             return true or trap()\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");

    assert_eq!(
        interpreter
            .call(mir.functions()[1].symbol(), &[])
            .expect("false and short-circuits"),
        vec![MirValue::Boolean(false)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[2].symbol(), &[])
            .expect("true or short-circuits"),
        vec![MirValue::Boolean(true)]
    );
}

#[test]
fn optional_flow_distinguishes_absent_from_present_false_and_zero() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function choose(value: Int?, fallback: Int): Int\n\
             return value ?? fallback\n\
         end\n\
         public function isPresent(value: Boolean?): Int\n\
             if local present = value then\n\
                 return 1\n\
             end\n\
             return 0\n\
         end\n\
         public function propagate(value: Int?): Int?\n\
             value?\n\
             return value\n\
         end\n\
         private function trapDefault(): Int\n\
             return 1 / 0\n\
         end\n\
         public function lazy(value: Int?): Int\n\
             return value ?? trapDefault()\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified optional MIR");

    assert_eq!(
        interpreter
            .call(mir.functions()[0].symbol(), &[int(0), int(7)])
            .expect("present zero"),
        vec![int(0)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[0].symbol(), &[MirValue::Nil, int(7)])
            .expect("absent default"),
        vec![int(7)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[1].symbol(), &[MirValue::Boolean(false)],)
            .expect("present false"),
        vec![int(1)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[1].symbol(), &[MirValue::Nil])
            .expect("absent Boolean"),
        vec![int(0)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[2].symbol(), &[MirValue::Nil])
            .expect("propagated absence"),
        vec![MirValue::Nil]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[2].symbol(), &[int(0)])
            .expect("propagated presence"),
        vec![int(0)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[4].symbol(), &[int(0)])
            .expect("present value skips fallback"),
        vec![int(0)]
    );
    assert_eq!(
        interpreter
            .call(mir.functions()[4].symbol(), &[MirValue::Nil])
            .expect_err("absent value evaluates fallback"),
        trap(TrapKind::DivisionByZero)
    );

    let optimized = optimize_mir(mir, &types).expect("optimized optional MIR");
    let optimized_interpreter =
        MirInterpreter::new(&optimized, &types).expect("verified optimized optional MIR");
    assert_eq!(
        optimized_interpreter
            .call(optimized.functions()[0].symbol(), &[int(0), int(7)])
            .expect("optimized present zero"),
        vec![int(0)]
    );
    assert_eq!(
        optimized_interpreter
            .call(optimized.functions()[0].symbol(), &[MirValue::Nil, int(7)])
            .expect("optimized absent default"),
        vec![int(7)]
    );
}

#[test]
fn zero_result_calls_execute_for_every_resolved_dispatch_kind() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         private function observe(value: Int)\n\
             value + 1\n\
         end\n\
         private function apply(operation: function(value: Int), value: Int)\n\
             operation(value)\n\
         end\n\
         public class Connection\n\
             private closed: Boolean = false\n\
             public function Connection:close()\n\
                 self.closed = true\n\
             end\n\
             public function Connection:isClosed(): Boolean\n\
                 return self.closed\n\
             end\n\
             public function Connection.reopen(connection: Connection)\n\
                 connection.closed = false\n\
             end\n\
         end\n\
         public function run(): Boolean\n\
             local operation: function(value: Int) = observe\n\
             apply(operation, 1)\n\
             operation(2)\n\
             local connection = Connection {}\n\
             connection:close()\n\
             Connection.reopen(connection)\n\
             connection:close()\n\
             return connection:isClosed()\n\
         end\n",
    );
    let run = mir.functions()[2].symbol();

    assert_eq!(
        MirInterpreter::new(&mir, &types)
            .expect("verified MIR")
            .call(run, &[])
            .expect("zero-result calls"),
        vec![MirValue::Boolean(true)]
    );
}

fn integer(text: &str, kind: IntegerKind) -> MirValue {
    MirValue::Integer(IntegerValue::parse_decimal(text, kind).expect("integer test value"))
}

fn int(value: i64) -> MirValue {
    integer(&value.to_string(), IntegerKind::Int64)
}

fn float(text: &str, kind: FloatKind) -> MirValue {
    MirValue::Float(FloatValue::parse_decimal(text, kind).expect("float test value"))
}

#[test]
fn exact_numeric_kinds_execute_checked_and_ieee_semantics() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function addByte(left: UInt8, right: UInt8): UInt8\n\
             return left + right\n\
         end\n\
         public function lessUnsigned(left: UInt64, right: UInt64): Boolean\n\
             return left < right\n\
         end\n\
         public function addSingle(left: Float32, right: Float32): Float32\n\
             return left + right\n\
         end\n\
         public function divideDouble(left: Float64, right: Float64): Float64\n\
             return left / right\n\
         end\n\
         public function identityByte(value: UInt8): UInt8\n\
             return value\n\
         end\n\
         public function identitySingle(value: Float32): Float32\n\
             return value\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");

    assert_eq!(
        interpreter
            .call(
                mir.functions()[0].symbol(),
                &[
                    integer("254", IntegerKind::UInt8),
                    integer("1", IntegerKind::UInt8),
                ],
            )
            .expect("UInt8 add"),
        vec![integer("255", IntegerKind::UInt8)]
    );
    assert_eq!(
        interpreter.call(
            mir.functions()[0].symbol(),
            &[
                integer("255", IntegerKind::UInt8),
                integer("1", IntegerKind::UInt8),
            ],
        ),
        Err(trap(TrapKind::IntegerOverflow))
    );
    assert_eq!(
        interpreter
            .call(
                mir.functions()[1].symbol(),
                &[
                    integer("9223372036854775808", IntegerKind::UInt64),
                    integer("18446744073709551615", IntegerKind::UInt64),
                ],
            )
            .expect("UInt64 comparison"),
        vec![MirValue::Boolean(true)]
    );

    let single = interpreter
        .call(
            mir.functions()[2].symbol(),
            &[
                float("16777216", FloatKind::Float32),
                float("1", FloatKind::Float32),
            ],
        )
        .expect("Float32 rounding");
    assert_eq!(single, vec![float("16777216", FloatKind::Float32)]);

    let divided = interpreter
        .call(
            mir.functions()[3].symbol(),
            &[
                float("1", FloatKind::Float64),
                float("0", FloatKind::Float64),
            ],
        )
        .expect("IEEE zero division");
    let MirValue::Float(value) = divided[0] else {
        panic!("float result");
    };
    assert!(value.as_f64().is_infinite());

    assert_eq!(
        interpreter.call(
            mir.functions()[4].symbol(),
            &[integer("1", IntegerKind::Int16)],
        ),
        Err(ExecutionError::TypeMismatch)
    );
    assert_eq!(
        interpreter.call(
            mir.functions()[5].symbol(),
            &[float("1", FloatKind::Float64)],
        ),
        Err(ExecutionError::TypeMismatch)
    );
}

#[test]
fn remaining_exact_numeric_operations_preserve_width_and_format() {
    let (mir, types) = executable_source(
        "namespace Main\n\
         public function integerOperations(left: Int16, right: Int16): (Int16, Int16, Int16, Int16, Int16)\n\
             return (left - right, left * right, left / right, left % right, -left)\n\
         end\n\
         public function floatOperations(left: Float64, right: Float64): (Float64, Float64, Float64, Boolean, Boolean)\n\
             return (left - right, left * right, -left, left < right, left > right)\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");

    assert_eq!(
        interpreter
            .call(
                mir.functions()[0].symbol(),
                &[
                    integer("7", IntegerKind::Int16),
                    integer("2", IntegerKind::Int16),
                ],
            )
            .expect("remaining integer operations"),
        vec![MirValue::Tuple(vec![
            integer("5", IntegerKind::Int16),
            integer("14", IntegerKind::Int16),
            integer("3", IntegerKind::Int16),
            integer("1", IntegerKind::Int16),
            integer("-7", IntegerKind::Int16),
        ])]
    );
    assert_eq!(
        interpreter
            .call(
                mir.functions()[1].symbol(),
                &[
                    float("6", FloatKind::Float64),
                    float("2", FloatKind::Float64),
                ],
            )
            .expect("remaining float operations"),
        vec![MirValue::Tuple(vec![
            float("4", FloatKind::Float64),
            float("12", FloatKind::Float64),
            float("-6", FloatKind::Float64),
            MirValue::Boolean(false),
            MirValue::Boolean(true),
        ])]
    );
}
