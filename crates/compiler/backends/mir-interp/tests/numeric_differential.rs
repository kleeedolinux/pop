use std::collections::BTreeSet;

use pop_backend_mir_interp::{ExecutionError, MirInterpreter, MirValue};
use pop_compile_time::{
    CompileTimeBinaryOperator, CompileTimeExpression, CompileTimeFunction, CompileTimeInterpreter,
    CompileTimeProgram, CompileTimeValue, EvaluationError,
};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, FrontEndResult, analyze_bubble};
use pop_foundation::{
    BubbleId, FileId, FunctionId, ModuleId, NamespaceId, SourceSpan, TextRange, TextSize,
};
use pop_mir::{MirBubble, lower_hir_bubble, optimize_mir};
use pop_query::QueryBudget;
use pop_runtime_interface::{RuntimeFailure, Trap, TrapKind};
use pop_source::SourceFile;
use pop_types::{IntegerKind, IntegerValue, TypeArena};

const INTEGER_KINDS: [(&str, IntegerKind, &str); 8] = [
    ("Int8", IntegerKind::Int8, "127"),
    ("Int16", IntegerKind::Int16, "32767"),
    ("Int32", IntegerKind::Int32, "2147483647"),
    ("Int64", IntegerKind::Int64, "9223372036854775807"),
    ("UInt8", IntegerKind::UInt8, "255"),
    ("UInt16", IntegerKind::UInt16, "65535"),
    ("UInt32", IntegerKind::UInt32, "4294967295"),
    ("UInt64", IntegerKind::UInt64, "18446744073709551615"),
];

fn trap(kind: TrapKind) -> ExecutionError {
    ExecutionError::Runtime(RuntimeFailure::Trap(Trap::new(kind)))
}

fn span() -> SourceSpan {
    SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)))
}

fn analyze(text: &str) -> FrontEndResult {
    let source = SourceFile::new(FileId::from_raw(0), "src/numeric.pop", text).expect("source");
    analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ))
}

fn lower(text: &str) -> (MirBubble, TypeArena) {
    let front_end = analyze(text);
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    (mir, front_end.types().clone())
}

fn integer(text: &str, kind: IntegerKind) -> IntegerValue {
    IntegerValue::parse_decimal(text, kind).expect("integer")
}

fn compile_time_binary(
    name: &str,
    kind: IntegerKind,
    operator: CompileTimeBinaryOperator,
    left: &str,
    right: &str,
) -> Result<CompileTimeValue, EvaluationError> {
    let arena = TypeArena::new();
    let numeric_type = arena.source_type(name).expect("numeric type");
    let constant = |text| {
        CompileTimeExpression::constant(
            CompileTimeValue::Integer(integer(text, kind)),
            numeric_type,
            span(),
        )
    };
    let function = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        numeric_type,
        CompileTimeExpression::binary(
            operator,
            constant(left),
            constant(right),
            numeric_type,
            span(),
        ),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("compile-time program");
    let eligible = BTreeSet::from([FunctionId::from_raw(0)]);
    CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
        .evaluate(FunctionId::from_raw(0), &[])
        .map(|result| result.value().clone())
}

#[test]
fn integer_success_is_identical_in_defaults_compile_time_mir_and_optimized_mir() {
    for (name, kind, _) in INTEGER_KINDS {
        let source = format!(
            "namespace Main\n\
             public record NumericDefault\n\
                 value: {name} = 1 + 2\n\
             end\n\
             public function defaultValue(): {name}\n\
                 local value: NumericDefault = {{}}\n\
                 return value.value\n\
             end\n\
             public function add(left: {name}, right: {name}): {name}\n\
                 return left + right\n\
             end\n"
        );
        let (mir, types) = lower(&source);
        let optimized = optimize_mir(mir.clone(), &types).expect("optimized MIR");
        let expected = MirValue::Integer(integer("3", kind));
        let arguments = [
            MirValue::Integer(integer("1", kind)),
            MirValue::Integer(integer("2", kind)),
        ];

        assert_eq!(
            MirInterpreter::new(&mir, &types)
                .expect("verified MIR")
                .call(mir.functions()[0].symbol(), &[]),
            Ok(vec![expected.clone()]),
            "field default for {name}"
        );
        assert_eq!(
            MirInterpreter::new(&mir, &types)
                .expect("verified MIR")
                .call(mir.functions()[1].symbol(), &arguments),
            Ok(vec![expected.clone()]),
            "MIR for {name}"
        );
        assert_eq!(
            MirInterpreter::new(&optimized, &types)
                .expect("verified optimized MIR")
                .call(optimized.functions()[1].symbol(), &arguments),
            Ok(vec![expected]),
            "optimized MIR for {name}"
        );
        assert_eq!(
            compile_time_binary(name, kind, CompileTimeBinaryOperator::CheckedAdd, "1", "2"),
            Ok(CompileTimeValue::Integer(integer("3", kind))),
            "compile time for {name}"
        );
    }
}

#[test]
fn integer_failures_are_identical_across_constant_and_executable_layers() {
    for (name, kind, maximum) in INTEGER_KINDS {
        assert_field_default_failure(name, maximum, "1", "+", "POP4002");
        assert_field_default_failure(name, "1", "0", "/", "POP4003");
        assert_compile_time_failure(name, kind, maximum);
        assert_mir_failures(name, kind, maximum);
    }
}

fn assert_field_default_failure(
    name: &str,
    left: &str,
    right: &str,
    operator: &str,
    expected_code: &str,
) {
    let source = format!(
        "namespace Main\n\
         public record InvalidDefault\n\
             value: {name} = {left} {operator} {right}\n\
         end\n"
    );
    let result = analyze(&source);
    assert!(
        result.hir().is_none(),
        "invalid {name} default published HIR"
    );
    assert_eq!(result.diagnostics().len(), 1, "{name}: {result:?}");
    assert_eq!(
        result.diagnostics()[0].code().as_str(),
        expected_code,
        "{name}"
    );
}

fn assert_compile_time_failure(name: &str, kind: IntegerKind, maximum: &str) {
    assert_eq!(
        compile_time_binary(
            name,
            kind,
            CompileTimeBinaryOperator::CheckedAdd,
            maximum,
            "1",
        ),
        Err(EvaluationError::IntegerOverflow),
        "compile-time overflow for {name}"
    );
    assert_eq!(
        compile_time_binary(
            name,
            kind,
            CompileTimeBinaryOperator::CheckedDivide,
            "1",
            "0",
        ),
        Err(EvaluationError::DivisionByZero),
        "compile-time division for {name}"
    );
}

fn assert_mir_failures(name: &str, kind: IntegerKind, maximum: &str) {
    let source = format!(
        "namespace Main\n\
         public function add(left: {name}, right: {name}): {name}\n\
             return left + right\n\
         end\n\
         public function divide(left: {name}, right: {name}): {name}\n\
             return left / right\n\
         end\n"
    );
    let (mir, types) = lower(&source);
    let optimized = optimize_mir(mir.clone(), &types).expect("optimized MIR");
    let maximum = MirValue::Integer(integer(maximum, kind));
    let one = MirValue::Integer(integer("1", kind));
    let zero = MirValue::Integer(integer("0", kind));

    for candidate in [&mir, &optimized] {
        let interpreter = MirInterpreter::new(candidate, &types).expect("verified MIR");
        assert_eq!(
            interpreter.call(
                candidate.functions()[0].symbol(),
                &[maximum.clone(), one.clone()]
            ),
            Err(trap(TrapKind::IntegerOverflow)),
            "MIR overflow for {name}"
        );
        assert_eq!(
            interpreter.call(
                candidate.functions()[1].symbol(),
                &[one.clone(), zero.clone()]
            ),
            Err(trap(TrapKind::DivisionByZero)),
            "MIR division for {name}"
        );
    }
}
