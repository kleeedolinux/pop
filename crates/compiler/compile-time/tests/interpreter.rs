use std::cmp::Ordering;
use std::collections::BTreeSet;

use pop_compile_time::{
    COMPILE_TIME_IR_VERSION, CompileTimeBinaryOperator, CompileTimeBudget, CompileTimeDependency,
    CompileTimeExpression, CompileTimeFunction, CompileTimeHandleKind, CompileTimeInterpreter,
    CompileTimeProgram, CompileTimeTypeMetadata, CompileTimeUnaryOperator, CompileTimeValue,
    DEFAULT_MAXIMUM_DIAGNOSTICS, DEFAULT_MAXIMUM_LIVE_VALUES, DEFAULT_MAXIMUM_OUTPUT_BYTES,
    EvaluationError, EvaluationFailureKind, ProgramError,
};
use pop_foundation::{
    FieldId, FileId, FunctionId, OpaqueId, SourceSpan, SymbolId, TextRange, TextSize, TypeId,
    UnionCaseId,
};
use pop_query::{BudgetError, QueryBudget};
use pop_types::{
    FloatKind, FloatValue, IntegerKind, IntegerValue, NumericConversionKind, SemanticType,
    TypeArena,
};

fn span() -> SourceSpan {
    SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)))
}

fn integer(text: &str, kind: IntegerKind) -> CompileTimeValue {
    CompileTimeValue::Integer(IntegerValue::parse_decimal(text, kind).expect("integer value"))
}

fn float(text: &str, kind: FloatKind) -> CompileTimeValue {
    CompileTimeValue::Float(FloatValue::parse_decimal(text, kind).expect("float value"))
}

fn constant(value: CompileTimeValue, type_id: TypeId) -> CompileTimeExpression {
    CompileTimeExpression::constant(value, type_id, span())
}

fn assert_checked_integer_error(
    name: &str,
    operation: CompileTimeBinaryOperator,
    left: CompileTimeValue,
    right: CompileTimeValue,
    expected: EvaluationError,
) {
    let arena = TypeArena::new();
    let numeric_type = arena.source_type(name).expect("numeric type");
    let function = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        numeric_type,
        CompileTimeExpression::binary(
            operation,
            constant(left, numeric_type),
            constant(right, numeric_type),
            numeric_type,
            span(),
        ),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified program");
    let eligible = BTreeSet::from([FunctionId::from_raw(0)]);
    let error = CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
        .evaluate(FunctionId::from_raw(0), &[])
        .expect_err("checked operation must fail");

    assert_eq!(error, expected, "{name}");
}

fn arithmetic_program() -> (CompileTimeProgram, TypeId) {
    let arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let add_one = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        vec![int],
        int,
        CompileTimeExpression::binary(
            CompileTimeBinaryOperator::CheckedAdd,
            CompileTimeExpression::parameter(0, int, span()),
            constant(integer("1", IntegerKind::Int64), int),
            int,
            span(),
        ),
    );
    let caller = CompileTimeFunction::new(
        FunctionId::from_raw(1),
        Vec::new(),
        int,
        CompileTimeExpression::call(
            FunctionId::from_raw(0),
            vec![constant(integer("41", IntegerKind::Int64), int)],
            int,
            span(),
        ),
    );
    (
        CompileTimeProgram::new(vec![caller, add_one], &arena).expect("program"),
        int,
    )
}

struct CanonicalValueFixture {
    arena: TypeArena,
    int: TypeId,
    string: TypeId,
    array: TypeId,
    record: TypeId,
    union_symbol: SymbolId,
    union: TypeId,
    type_handle: TypeId,
    symbol_handle: TypeId,
    count: FieldId,
    name: FieldId,
    some: UnionCaseId,
    known_symbol: SymbolId,
    metadata: CompileTimeTypeMetadata,
}

fn canonical_value_fixture() -> CanonicalValueFixture {
    let mut arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let array = arena.intern(SemanticType::Array(int)).expect("array type");
    let record = arena
        .intern(SemanticType::Record(vec![
            ("count".to_owned(), int),
            ("name".to_owned(), string),
        ]))
        .expect("record type");
    let union_symbol = SymbolId::from_raw(40);
    let union = arena
        .intern(SemanticType::TaggedUnion {
            definition: union_symbol,
            source: union_symbol,
            arguments: Vec::new(),
        })
        .expect("union type");
    let type_handle = arena
        .intern(SemanticType::Opaque(OpaqueId::from_raw(0)))
        .expect("type handle");
    let symbol_handle = arena
        .intern(SemanticType::Opaque(OpaqueId::from_raw(1)))
        .expect("symbol handle");
    let count = FieldId::from_raw(10);
    let name = FieldId::from_raw(11);
    let some = UnionCaseId::from_raw(20);
    let known_symbol = SymbolId::from_raw(50);
    let metadata = CompileTimeTypeMetadata::new()
        .with_handle_type(type_handle, CompileTimeHandleKind::Type)
        .with_handle_type(symbol_handle, CompileTimeHandleKind::Symbol)
        .with_type(int)
        .with_symbol(known_symbol)
        .with_record(record, vec![(count, int), (name, string)])
        .with_union_case(union_symbol, some, vec![int]);
    CanonicalValueFixture {
        arena,
        int,
        string,
        array,
        record,
        union_symbol,
        union,
        type_handle,
        symbol_handle,
        count,
        name,
        some,
        known_symbol,
        metadata,
    }
}

#[test]
fn evaluation_is_deterministic_typed_and_independent_of_insertion_order() {
    let (program, int) = arithmetic_program();
    let eligible = BTreeSet::from([FunctionId::from_raw(0), FunctionId::from_raw(1)]);
    let budget = QueryBudget::new(100, 1024, 8);

    let first = CompileTimeInterpreter::new(&program, &eligible, budget)
        .evaluate(FunctionId::from_raw(1), &[])
        .expect("evaluation");
    let second = CompileTimeInterpreter::new(&program, &eligible, budget)
        .evaluate(FunctionId::from_raw(1), &[])
        .expect("evaluation");

    assert_eq!(first.value(), &integer("42", IntegerKind::Int64));
    assert_eq!(first, second);
    assert_eq!(
        first.function_dependencies(),
        &[FunctionId::from_raw(0), FunctionId::from_raw(1)]
    );
    assert!(
        first
            .dependencies()
            .contains(&CompileTimeDependency::Type(int))
    );
    assert_eq!(first.usage().instructions(), 5);
    assert_eq!(first.usage().allocated_bytes(), 16);
    assert_eq!(first.usage().maximum_call_depth(), 2);
    assert_eq!(
        first.budget().maximum_live_values(),
        DEFAULT_MAXIMUM_LIVE_VALUES
    );
    assert_eq!(
        first.budget().maximum_output_bytes(),
        DEFAULT_MAXIMUM_OUTPUT_BYTES
    );
    assert_eq!(
        first.budget().maximum_diagnostics(),
        DEFAULT_MAXIMUM_DIAGNOSTICS
    );
}

#[test]
fn canonical_aggregate_and_handle_values_require_complete_typed_metadata() {
    let fixture = canonical_value_fixture();

    let values = [
        (
            fixture.array,
            CompileTimeValue::Array(vec![integer("1", IntegerKind::Int64)]),
        ),
        (
            fixture.record,
            CompileTimeValue::Record(vec![
                (fixture.count, integer("1", IntegerKind::Int64)),
                (fixture.name, CompileTimeValue::String("Pop".to_owned())),
            ]),
        ),
        (
            fixture.union,
            CompileTimeValue::Union {
                union: fixture.union_symbol,
                case: fixture.some,
                arguments: vec![integer("1", IntegerKind::Int64)],
            },
        ),
        (
            fixture.type_handle,
            CompileTimeValue::TypeReference(fixture.int),
        ),
        (
            fixture.symbol_handle,
            CompileTimeValue::SymbolReference(fixture.known_symbol),
        ),
    ];
    let functions: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(index, (type_id, value))| {
            CompileTimeFunction::new(
                FunctionId::from_raw(u32::try_from(index).expect("small test index")),
                Vec::new(),
                *type_id,
                constant(value.clone(), *type_id),
            )
        })
        .collect();
    let program =
        CompileTimeProgram::new_with_metadata(functions, &fixture.arena, fixture.metadata.clone())
            .expect("fully described values");
    let eligible = program
        .functions()
        .iter()
        .map(CompileTimeFunction::function)
        .collect();
    let result = CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 4096, 8))
        .evaluate(FunctionId::from_raw(2), &[])
        .expect("union value");
    assert!(
        result
            .dependencies()
            .contains(&CompileTimeDependency::Type(fixture.union))
    );
    assert!(
        result
            .dependencies()
            .contains(&CompileTimeDependency::UnionCase {
                union: fixture.union_symbol,
                case: fixture.some,
            })
    );
    let type_result =
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 4096, 8))
            .evaluate(FunctionId::from_raw(3), &[])
            .expect("type handle");
    assert!(
        type_result
            .dependencies()
            .contains(&CompileTimeDependency::Type(fixture.int))
    );
    let symbol_result =
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 4096, 8))
            .evaluate(FunctionId::from_raw(4), &[])
            .expect("symbol handle");
    assert!(
        symbol_result
            .dependencies()
            .contains(&CompileTimeDependency::Symbol(fixture.known_symbol))
    );
}

#[test]
fn malformed_or_unowned_recursive_compile_time_values_are_rejected() {
    let fixture = canonical_value_fixture();
    let invalid_values = [
        (
            fixture.int,
            CompileTimeValue::TypeReference(fixture.int),
            CompileTimeTypeMetadata::new(),
        ),
        (
            fixture.type_handle,
            CompileTimeValue::TypeReference(fixture.string),
            CompileTimeTypeMetadata::new()
                .with_handle_type(fixture.type_handle, CompileTimeHandleKind::Type)
                .with_type(fixture.int),
        ),
        (
            fixture.symbol_handle,
            CompileTimeValue::SymbolReference(SymbolId::from_raw(999)),
            CompileTimeTypeMetadata::new()
                .with_handle_type(fixture.symbol_handle, CompileTimeHandleKind::Symbol),
        ),
        (
            fixture.array,
            CompileTimeValue::Array(vec![CompileTimeValue::String("wrong".to_owned())]),
            CompileTimeTypeMetadata::new(),
        ),
        (
            fixture.record,
            CompileTimeValue::Record(vec![
                (fixture.name, CompileTimeValue::String("Pop".to_owned())),
                (fixture.count, integer("1", IntegerKind::Int64)),
            ]),
            CompileTimeTypeMetadata::new().with_record(
                fixture.record,
                vec![(fixture.count, fixture.int), (fixture.name, fixture.string)],
            ),
        ),
        (
            fixture.union,
            CompileTimeValue::Union {
                union: fixture.union_symbol,
                case: UnionCaseId::from_raw(999),
                arguments: vec![integer("1", IntegerKind::Int64)],
            },
            CompileTimeTypeMetadata::new().with_union_case(
                fixture.union_symbol,
                fixture.some,
                vec![fixture.int],
            ),
        ),
    ];
    for (index, (type_id, value, metadata)) in invalid_values.into_iter().enumerate() {
        let function = CompileTimeFunction::new(
            FunctionId::from_raw(u32::try_from(index).expect("small test index")),
            Vec::new(),
            type_id,
            constant(value, type_id),
        );
        assert!(matches!(
            CompileTimeProgram::new_with_metadata(vec![function], &fixture.arena, metadata),
            Err(ProgramError::ValueTypeMismatch { expected }) if expected == type_id
        ));
    }
}

#[test]
fn optional_compile_time_values_accept_nil_or_the_recursive_inner_shape() {
    let mut arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let optional = arena
        .intern(SemanticType::Optional(int))
        .expect("optional type");
    let functions = vec![
        CompileTimeFunction::new(
            FunctionId::from_raw(0),
            Vec::new(),
            optional,
            constant(CompileTimeValue::Nil, optional),
        ),
        CompileTimeFunction::new(
            FunctionId::from_raw(1),
            Vec::new(),
            optional,
            constant(integer("42", IntegerKind::Int64), optional),
        ),
    ];
    let program = CompileTimeProgram::new(functions, &arena).expect("optional values");
    let eligible = BTreeSet::from([FunctionId::from_raw(0), FunctionId::from_raw(1)]);

    assert_eq!(
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
            .evaluate(FunctionId::from_raw(0), &[])
            .expect("nil optional")
            .value(),
        &CompileTimeValue::Nil
    );
    assert_eq!(
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
            .evaluate(FunctionId::from_raw(1), &[])
            .expect("present optional")
            .value(),
        &integer("42", IntegerKind::Int64)
    );
}

#[test]
fn canonical_arguments_are_explicit_transitive_dependencies() {
    let (program, _) = arithmetic_program();
    let eligible = BTreeSet::from([FunctionId::from_raw(0), FunctionId::from_raw(1)]);
    let result = CompileTimeInterpreter::new(
        &program,
        &eligible,
        CompileTimeBudget::new(QueryBudget::new(100, 1024, 8), 32, 1024, 4),
    )
    .evaluate(FunctionId::from_raw(1), &[])
    .expect("evaluation");

    assert!(
        result
            .dependencies()
            .contains(&CompileTimeDependency::CanonicalArguments {
                function: FunctionId::from_raw(1),
                arguments: Vec::new(),
            })
    );
    assert!(
        result
            .dependencies()
            .contains(&CompileTimeDependency::Compiler {
                compiler_version: env!("CARGO_PKG_VERSION"),
                compile_time_ir_version: COMPILE_TIME_IR_VERSION,
            })
    );
    assert!(
        result
            .dependencies()
            .contains(&CompileTimeDependency::CanonicalArguments {
                function: FunctionId::from_raw(0),
                arguments: vec![integer("41", IntegerKind::Int64)],
            })
    );
    assert_eq!(result.evaluation_key().function(), FunctionId::from_raw(1));
    assert!(result.evaluation_key().arguments().is_empty());
}

#[test]
fn an_active_call_with_the_same_canonical_arguments_is_an_explicit_cycle() {
    let arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let recursive = FunctionId::from_raw(0);
    let function = CompileTimeFunction::new(
        recursive,
        Vec::new(),
        int,
        CompileTimeExpression::call(recursive, Vec::new(), int, span()),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified recursion");
    let eligible = BTreeSet::from([recursive]);
    let failure = CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
        .evaluate_detailed(recursive, &[])
        .expect_err("same evaluation key must cycle");

    assert_eq!(failure.kind(), EvaluationFailureKind::CallCycle);
    assert_eq!(
        failure
            .call_chain()
            .iter()
            .map(|frame| frame.function())
            .collect::<Vec<_>>(),
        [recursive, recursive]
    );
    assert_eq!(failure.location(), span());
    assert_eq!(failure.usage().maximum_call_depth(), 1);
    assert!(
        failure
            .dependencies()
            .contains(&CompileTimeDependency::Function(recursive))
    );
    assert!(
        failure
            .dependencies()
            .contains(&CompileTimeDependency::Type(int))
    );
}

#[test]
fn detailed_failures_retain_the_request_origin_key_and_complete_call_chain() {
    let arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let recursive = FunctionId::from_raw(0);
    let call_site = SourceSpan::new(
        FileId::from_raw(1),
        TextRange::empty(TextSize::from_u32(12)),
    );
    let origin = SourceSpan::new(
        FileId::from_raw(2),
        TextRange::empty(TextSize::from_u32(34)),
    );
    let function = CompileTimeFunction::new(
        recursive,
        vec![int],
        int,
        CompileTimeExpression::call(
            recursive,
            vec![CompileTimeExpression::parameter(0, int, call_site)],
            int,
            call_site,
        ),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified recursion");
    let eligible = BTreeSet::from([recursive]);
    let argument = integer("7", IntegerKind::Int64);
    let failure = CompileTimeInterpreter::new(
        &program,
        &eligible,
        CompileTimeBudget::new(QueryBudget::new(100, 1024, 8), 32, 1024, 4),
    )
    .evaluate_detailed_from(recursive, std::slice::from_ref(&argument), origin)
    .expect_err("same canonical request must cycle");

    assert_eq!(failure.kind(), EvaluationFailureKind::CallCycle);
    assert_eq!(failure.origin(), origin);
    assert_eq!(failure.evaluation_key().function(), recursive);
    assert_eq!(failure.evaluation_key().arguments(), &[argument]);
    assert_eq!(failure.location(), call_site);
    assert_eq!(failure.call_chain().len(), 2);
    assert!(
        failure
            .call_chain()
            .iter()
            .all(|frame| frame.call_site() == call_site)
    );
}

#[test]
fn recursive_calls_with_distinct_canonical_arguments_remain_budgeted_computation() {
    let arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let countdown = FunctionId::from_raw(0);
    let parameter = CompileTimeExpression::parameter(0, int, span());
    let condition = CompileTimeExpression::binary(
        CompileTimeBinaryOperator::Equal,
        parameter.clone(),
        constant(integer("0", IntegerKind::Int64), int),
        boolean,
        span(),
    );
    let next = CompileTimeExpression::binary(
        CompileTimeBinaryOperator::CheckedSubtract,
        parameter,
        constant(integer("1", IntegerKind::Int64), int),
        int,
        span(),
    );
    let function = CompileTimeFunction::new(
        countdown,
        vec![int],
        int,
        CompileTimeExpression::conditional(
            condition,
            constant(integer("0", IntegerKind::Int64), int),
            CompileTimeExpression::call(countdown, vec![next], int, span()),
            int,
            span(),
        ),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified recursion");
    let eligible = BTreeSet::from([countdown]);
    let result = CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
        .evaluate_detailed(countdown, &[integer("3", IntegerKind::Int64)])
        .expect("decreasing recursive evaluation");

    assert_eq!(result.value(), &integer("0", IntegerKind::Int64));
    assert_eq!(result.usage().maximum_call_depth(), 4);
}

#[test]
fn eligibility_and_every_resource_budget_are_enforced_before_publication() {
    let (program, _) = arithmetic_program();
    let all = BTreeSet::from([FunctionId::from_raw(0), FunctionId::from_raw(1)]);
    let only_caller = BTreeSet::from([FunctionId::from_raw(1)]);

    let forbidden =
        CompileTimeInterpreter::new(&program, &only_caller, QueryBudget::new(100, 1024, 8))
            .evaluate(FunctionId::from_raw(1), &[])
            .expect_err("callee is not compile-time eligible");
    assert_eq!(
        forbidden,
        EvaluationError::IneligibleFunction(FunctionId::from_raw(0))
    );

    for (budget, expected) in [
        (QueryBudget::new(1, 1024, 8), BudgetError::InstructionLimit),
        (QueryBudget::new(100, 0, 8), BudgetError::AllocationLimit),
        (QueryBudget::new(100, 1024, 1), BudgetError::CallDepthLimit),
    ] {
        let error = CompileTimeInterpreter::new(&program, &all, budget)
            .evaluate(FunctionId::from_raw(1), &[])
            .expect_err("budget must be enforced");
        assert_eq!(error, EvaluationError::Budget(expected));
    }

    for (budget, expected) in [
        (
            CompileTimeBudget::new(QueryBudget::new(100, 1024, 8), 0, 1024, 4),
            BudgetError::LiveValueLimit,
        ),
        (
            CompileTimeBudget::new(QueryBudget::new(100, 1024, 8), 32, 7, 4),
            BudgetError::OutputSizeLimit,
        ),
    ] {
        let failure = CompileTimeInterpreter::new(&program, &all, budget)
            .evaluate_detailed(FunctionId::from_raw(1), &[])
            .expect_err("compile-time-specific budget must be enforced");
        assert_eq!(
            failure.kind(),
            EvaluationFailureKind::Error(EvaluationError::Budget(expected))
        );
        assert_eq!(failure.budget(), &budget);
    }

    let diagnostic_budget = CompileTimeBudget::new(QueryBudget::new(100, 1024, 8), 32, 1024, 0);
    let diagnostic_failure = CompileTimeInterpreter::new(&program, &all, diagnostic_budget)
        .with_recorded_diagnostics(1)
        .evaluate_detailed(FunctionId::from_raw(1), &[])
        .expect_err("diagnostic budget must be enforced");
    assert_eq!(
        diagnostic_failure.kind(),
        EvaluationFailureKind::Error(EvaluationError::Budget(BudgetError::DiagnosticLimit))
    );
    assert_eq!(diagnostic_failure.usage().diagnostics(), 1);

    let budget = CompileTimeBudget::new(QueryBudget::new(100, 1024, 8), 32, 1024, 4);
    let result = CompileTimeInterpreter::new(&program, &all, budget)
        .evaluate(FunctionId::from_raw(1), &[])
        .expect("budgeted evaluation");
    assert_eq!(result.budget(), &budget);
    assert!(result.usage().maximum_live_values() > 0);
    assert_eq!(result.usage().output_bytes(), 8);
    assert_eq!(result.usage().diagnostics(), 0);
}

#[test]
fn aggregate_temporaries_cannot_bypass_the_live_value_budget() {
    let mut arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let pair = arena
        .intern(SemanticType::Tuple(vec![int, int]))
        .expect("pair type");
    let pair_expression = || {
        CompileTimeExpression::tuple(
            vec![
                constant(integer("1", IntegerKind::Int64), int),
                constant(integer("2", IntegerKind::Int64), int),
            ],
            pair,
            span(),
        )
    };
    let function = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        boolean,
        CompileTimeExpression::binary(
            CompileTimeBinaryOperator::Equal,
            pair_expression(),
            pair_expression(),
            boolean,
            span(),
        ),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified program");
    let eligible = BTreeSet::from([FunctionId::from_raw(0)]);
    let budget = CompileTimeBudget::new(QueryBudget::new(100, 1024, 8), 5, 1024, 4);

    let failure = CompileTimeInterpreter::new(&program, &eligible, budget)
        .evaluate_detailed(FunctionId::from_raw(0), &[])
        .expect_err("both live aggregate operands must count toward the limit");

    assert_eq!(
        failure.kind(),
        EvaluationFailureKind::Error(EvaluationError::Budget(BudgetError::LiveValueLimit))
    );
    assert_eq!(failure.usage().maximum_live_values(), 6);
}

#[test]
fn checked_integer_semantics_use_every_declared_width_and_signedness() {
    for (name, kind, maximum) in [
        ("Int8", IntegerKind::Int8, "127"),
        ("Int16", IntegerKind::Int16, "32767"),
        ("Int32", IntegerKind::Int32, "2147483647"),
        ("Int64", IntegerKind::Int64, "9223372036854775807"),
        ("UInt8", IntegerKind::UInt8, "255"),
        ("UInt16", IntegerKind::UInt16, "65535"),
        ("UInt32", IntegerKind::UInt32, "4294967295"),
        ("UInt64", IntegerKind::UInt64, "18446744073709551615"),
    ] {
        assert_checked_integer_error(
            name,
            CompileTimeBinaryOperator::CheckedAdd,
            integer(maximum, kind),
            integer("1", kind),
            EvaluationError::IntegerOverflow,
        );
    }

    for (name, operation, left, right, expected) in [
        (
            "Int8",
            CompileTimeBinaryOperator::CheckedMultiply,
            integer("64", IntegerKind::Int8),
            integer("2", IntegerKind::Int8),
            EvaluationError::IntegerOverflow,
        ),
        (
            "UInt8",
            CompileTimeBinaryOperator::CheckedSubtract,
            integer("0", IntegerKind::UInt8),
            integer("1", IntegerKind::UInt8),
            EvaluationError::IntegerOverflow,
        ),
        (
            "Int64",
            CompileTimeBinaryOperator::CheckedDivide,
            integer("-9223372036854775808", IntegerKind::Int64),
            integer("-1", IntegerKind::Int64),
            EvaluationError::IntegerOverflow,
        ),
        (
            "UInt64",
            CompileTimeBinaryOperator::CheckedDivide,
            integer("18446744073709551615", IntegerKind::UInt64),
            integer("0", IntegerKind::UInt64),
            EvaluationError::DivisionByZero,
        ),
        (
            "UInt16",
            CompileTimeBinaryOperator::CheckedRemainder,
            integer("1", IntegerKind::UInt16),
            integer("0", IntegerKind::UInt16),
            EvaluationError::DivisionByZero,
        ),
    ] {
        assert_checked_integer_error(name, operation, left, right, expected);
    }
}

#[test]
fn unsigned_comparison_preserves_values_above_i64_max() {
    let arena = TypeArena::new();
    let uint64 = arena.source_type("UInt64").expect("UInt64");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let function = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        boolean,
        CompileTimeExpression::binary(
            CompileTimeBinaryOperator::GreaterThan,
            constant(integer("18446744073709551615", IntegerKind::UInt64), uint64),
            constant(integer("9223372036854775808", IntegerKind::UInt64), uint64),
            boolean,
            span(),
        ),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified program");
    let eligible = BTreeSet::from([FunctionId::from_raw(0)]);

    assert_eq!(
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
            .evaluate(FunctionId::from_raw(0), &[])
            .expect("comparison")
            .value(),
        &CompileTimeValue::Boolean(true)
    );
}

#[test]
fn float_evaluation_preserves_ieee_width_without_adding_float_equality() {
    for (name, kind, expected) in [
        ("Float32", FloatKind::Float32, "16777216"),
        ("Float64", FloatKind::Float64, "16777217"),
    ] {
        let arena = TypeArena::new();
        let float_type = arena.source_type(name).expect("float type");
        let function = CompileTimeFunction::new(
            FunctionId::from_raw(0),
            Vec::new(),
            float_type,
            CompileTimeExpression::binary(
                CompileTimeBinaryOperator::FloatAdd,
                constant(float("16777216", kind), float_type),
                constant(float("1", kind), float_type),
                float_type,
                span(),
            ),
        );
        let program = CompileTimeProgram::new(vec![function], &arena).expect("verified program");
        let eligible = BTreeSet::from([FunctionId::from_raw(0)]);
        let result =
            CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
                .evaluate(FunctionId::from_raw(0), &[])
                .expect("float evaluation");
        let CompileTimeValue::Float(value) = result.value() else {
            panic!("float value");
        };
        assert_eq!(
            *value,
            FloatValue::parse_decimal(expected, kind).expect("expected float"),
            "{name}"
        );
    }
}

#[test]
fn explicit_numeric_conversions_and_complete_ordering_evaluate_at_compile_time() {
    let arena = TypeArena::new();
    let int = arena.source_type("Int").expect("Int");
    let uint8 = arena.source_type("UInt8").expect("UInt8");
    let float64 = arena.source_type("Float64").expect("Float64");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let conversion = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        int,
        CompileTimeExpression::numeric_convert(
            NumericConversionKind::FloatToInteger {
                source: FloatKind::Float64,
                target: IntegerKind::Int64,
            },
            constant(float("-12.75", FloatKind::Float64), float64),
            int,
            span(),
        ),
    );
    let ordering = CompileTimeFunction::new(
        FunctionId::from_raw(1),
        Vec::new(),
        boolean,
        CompileTimeExpression::binary(
            CompileTimeBinaryOperator::LessThanOrEqual,
            constant(float("1.5", FloatKind::Float64), float64),
            constant(float("1.5", FloatKind::Float64), float64),
            boolean,
            span(),
        ),
    );
    let invalid = CompileTimeFunction::new(
        FunctionId::from_raw(2),
        Vec::new(),
        uint8,
        CompileTimeExpression::numeric_convert(
            NumericConversionKind::IntegerToInteger {
                source: IntegerKind::Int64,
                target: IntegerKind::UInt8,
            },
            constant(integer("256", IntegerKind::Int64), int),
            uint8,
            span(),
        ),
    );
    let program = CompileTimeProgram::new(vec![conversion, ordering, invalid], &arena)
        .expect("verified conversions");
    let eligible = BTreeSet::from([
        FunctionId::from_raw(0),
        FunctionId::from_raw(1),
        FunctionId::from_raw(2),
    ]);
    let evaluate = |function| {
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
            .evaluate(FunctionId::from_raw(function), &[])
    };

    assert_eq!(
        evaluate(0).expect("conversion").value(),
        &integer("-12", IntegerKind::Int64)
    );
    assert_eq!(
        evaluate(1).expect("ordering").value(),
        &CompileTimeValue::Boolean(true)
    );
    assert_eq!(evaluate(2), Err(EvaluationError::IntegerOverflow));
}

#[test]
fn unary_numeric_operations_preserve_exact_kind_and_checked_failures() {
    let arena = TypeArena::new();
    let int8 = arena.source_type("Int8").expect("Int8");
    let float32 = arena.source_type("Float32").expect("Float32");
    let negate_float = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        float32,
        CompileTimeExpression::unary(
            CompileTimeUnaryOperator::FloatNegate,
            constant(float("1.5", FloatKind::Float32), float32),
            float32,
            span(),
        ),
    );
    let overflow = CompileTimeFunction::new(
        FunctionId::from_raw(1),
        Vec::new(),
        int8,
        CompileTimeExpression::unary(
            CompileTimeUnaryOperator::CheckedIntegerNegate,
            constant(integer("-128", IntegerKind::Int8), int8),
            int8,
            span(),
        ),
    );
    let program =
        CompileTimeProgram::new(vec![overflow, negate_float], &arena).expect("verified program");
    let eligible = BTreeSet::from([FunctionId::from_raw(0), FunctionId::from_raw(1)]);

    let result = CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
        .evaluate(FunctionId::from_raw(0), &[])
        .expect("float negate");
    assert_eq!(result.value(), &float("-1.5", FloatKind::Float32),);
    assert_eq!(
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8),)
            .evaluate(FunctionId::from_raw(1), &[]),
        Err(EvaluationError::IntegerOverflow)
    );
}

#[test]
fn verifier_rejects_numeric_value_and_operator_type_disagreement() {
    let arena = TypeArena::new();
    let int8 = arena.source_type("Int8").expect("Int8");
    let uint8 = arena.source_type("UInt8").expect("UInt8");
    let float32 = arena.source_type("Float32").expect("Float32");
    let float64 = arena.source_type("Float64").expect("Float64");

    let wrong_constant = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        uint8,
        constant(integer("1", IntegerKind::Int8), uint8),
    );
    assert!(matches!(
        CompileTimeProgram::new(vec![wrong_constant], &arena),
        Err(ProgramError::ValueTypeMismatch { expected }) if expected == uint8
    ));

    let wrong_float_constant = CompileTimeFunction::new(
        FunctionId::from_raw(1),
        Vec::new(),
        float64,
        constant(float("1", FloatKind::Float32), float64),
    );
    assert!(matches!(
        CompileTimeProgram::new(vec![wrong_float_constant], &arena),
        Err(ProgramError::ValueTypeMismatch { expected }) if expected == float64
    ));

    let mixed_integer_kinds = CompileTimeFunction::new(
        FunctionId::from_raw(2),
        Vec::new(),
        int8,
        CompileTimeExpression::binary(
            CompileTimeBinaryOperator::CheckedAdd,
            constant(integer("1", IntegerKind::Int8), int8),
            constant(integer("1", IntegerKind::UInt8), uint8),
            int8,
            span(),
        ),
    );
    assert!(matches!(
        CompileTimeProgram::new(vec![mixed_integer_kinds], &arena),
        Err(ProgramError::InvalidBinaryOperator {
            operator: CompileTimeBinaryOperator::CheckedAdd,
            ..
        })
    ));

    let wrong_float = CompileTimeFunction::new(
        FunctionId::from_raw(3),
        Vec::new(),
        float32,
        CompileTimeExpression::binary(
            CompileTimeBinaryOperator::FloatAdd,
            constant(integer("1", IntegerKind::Int8), int8),
            constant(integer("2", IntegerKind::Int8), int8),
            float32,
            span(),
        ),
    );
    assert!(matches!(
        CompileTimeProgram::new(vec![wrong_float], &arena),
        Err(ProgramError::InvalidBinaryOperator {
            operator: CompileTimeBinaryOperator::FloatAdd,
            ..
        })
    ));
}

#[test]
fn runtime_arguments_must_match_the_verified_numeric_parameter_type() {
    let arena = TypeArena::new();
    let int8 = arena.source_type("Int8").expect("Int8");
    let function = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        vec![int8],
        int8,
        CompileTimeExpression::parameter(0, int8, span()),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified program");
    let eligible = BTreeSet::from([FunctionId::from_raw(0)]);

    assert_eq!(
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8),)
            .evaluate(FunctionId::from_raw(0), &[integer("1", IntegerKind::UInt8)],),
        Err(EvaluationError::TypeMismatch)
    );
}

#[test]
fn integer_and_float_ordering_are_typed_and_nan_is_unordered() {
    let arena = TypeArena::new();
    let float64 = arena.source_type("Float64").expect("Float64");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let zero = FloatValue::parse_decimal("0", FloatKind::Float64).expect("zero");
    let nan = zero.checked_divide(zero).expect("IEEE NaN");
    assert_eq!(nan.partial_compare(zero), Ok(None));

    let function = CompileTimeFunction::new(
        FunctionId::from_raw(0),
        Vec::new(),
        boolean,
        CompileTimeExpression::binary(
            CompileTimeBinaryOperator::LessThan,
            constant(CompileTimeValue::Float(nan), float64),
            constant(CompileTimeValue::Float(zero), float64),
            boolean,
            span(),
        ),
    );
    let program = CompileTimeProgram::new(vec![function], &arena).expect("verified program");
    let eligible = BTreeSet::from([FunctionId::from_raw(0)]);
    assert_eq!(
        CompileTimeInterpreter::new(&program, &eligible, QueryBudget::new(100, 1024, 8))
            .evaluate(FunctionId::from_raw(0), &[])
            .expect("unordered comparison")
            .value(),
        &CompileTimeValue::Boolean(false)
    );

    let one = IntegerValue::parse_decimal("1", IntegerKind::Int8).expect("one");
    let two = IntegerValue::parse_decimal("2", IntegerKind::Int8).expect("two");
    assert_eq!(one.compare(two), Ok(Ordering::Less));
}
