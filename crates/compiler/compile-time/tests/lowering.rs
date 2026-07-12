use std::collections::{BTreeMap, BTreeSet};

use pop_compile_time::{
    CompileTimeInterpreter, CompileTimeLoweringError, CompileTimeProgram, CompileTimeValue,
    UnsupportedCompileTimeConstruct, lower_compile_time_expression, lower_compile_time_function,
};
use pop_foundation::{BubbleId, FileId, FunctionId, ModuleId};
use pop_query::QueryBudget;
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_class_declaration, parse_file, parse_function_body, parse_function_signature,
};
use pop_types::{
    BodyChecker, FloatKind, FloatValue, IntegerKind, IntegerValue, ResolvedFunctionSignature,
    SignatureResolver, TypeArena, TypedBody, TypedStatementKind, embedded_bootstrap_schema,
};

struct CheckedFunction {
    signature: ResolvedFunctionSignature,
    body: TypedBody,
}

struct CheckedProgram {
    arena: TypeArena,
    functions: BTreeMap<String, CheckedFunction>,
}

impl CheckedProgram {
    fn lower(&self, name: &str) -> pop_compile_time::CompileTimeFunction {
        let function = &self.functions[name];
        lower_compile_time_function(&function.signature, &function.body, &self.arena)
            .expect("eligible typed subset")
    }

    fn lower_error(&self, name: &str) -> CompileTimeLoweringError {
        let function = &self.functions[name];
        lower_compile_time_function(&function.signature, &function.body, &self.arena)
            .expect_err("construct must be rejected")
    }
}

fn check_program(source_text: &str) -> CheckedProgram {
    let module = ModuleId::from_raw(0);
    let source =
        SourceFile::new(FileId::from_raw(0), "src/compileTime.pop", source_text).expect("source");
    let syntax = parse_file(&source);
    assert!(
        syntax.diagnostics().is_empty(),
        "structural syntax: {:?}",
        syntax.diagnostics()
    );
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    assert!(indexed.diagnostics().is_empty(), "declaration index");
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));

    for node in syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::ClassDeclaration)
    {
        let class = parse_class_declaration(&source, &syntax, node).expect("class syntax");
        let qualified = format!("Example.{}", class.name());
        let symbol = database
            .index()
            .declaration_by_qualified_name(&qualified, SymbolSpace::Type)[0]
            .symbol();
        let result = resolver.define_class(module, symbol, &class);
        assert!(
            result.diagnostics().is_empty(),
            "{}",
            result.diagnostic_snapshot()
        );
    }

    let parsed: Vec<_> = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::FunctionDeclaration)
        .map(|node| {
            let signature =
                parse_function_signature(&source, &syntax, node).expect("function signature");
            let body = parse_function_body(&source, &syntax, node, &signature).expect("body");
            (signature, body)
        })
        .collect();
    let mut signatures = BTreeMap::new();
    let mut symbols = BTreeMap::new();
    for (syntax_signature, _) in &parsed {
        let qualified = format!("Example.{}", syntax_signature.name());
        let symbol = database
            .index()
            .declaration_by_qualified_name(&qualified, SymbolSpace::Value)[0]
            .symbol();
        let resolution = resolver.resolve(module, symbol, syntax_signature);
        assert!(
            resolution.diagnostics().is_empty(),
            "{}",
            resolution.diagnostic_snapshot()
        );
        signatures.insert(
            symbol,
            resolution.signature().expect("resolved signature").clone(),
        );
        symbols.insert(syntax_signature.name().to_owned(), symbol);
    }

    let mut functions = BTreeMap::new();
    for (syntax_signature, syntax_body) in &parsed {
        let symbol = symbols[syntax_signature.name()];
        let signature = signatures[&symbol].clone();
        let checked =
            BodyChecker::new(module, &mut resolver, &signatures).check(&signature, syntax_body);
        assert!(
            checked.diagnostics().is_empty(),
            "{}: {}",
            syntax_signature.name(),
            checked.diagnostic_snapshot()
        );
        functions.insert(
            syntax_signature.name().to_owned(),
            CheckedFunction {
                signature,
                body: checked.body().expect("typed body").clone(),
            },
        );
    }
    CheckedProgram {
        arena: resolver.into_arena(),
        functions,
    }
}

fn function_id(function: &CheckedFunction) -> FunctionId {
    FunctionId::from_raw(function.signature.symbol().raw())
}

fn evaluate(
    program: &CompileTimeProgram,
    eligible: &BTreeSet<FunctionId>,
    function: FunctionId,
    arguments: &[CompileTimeValue],
) -> CompileTimeValue {
    CompileTimeInterpreter::new(program, eligible, QueryBudget::new(10_000, 10_000, 32))
        .evaluate(function, arguments)
        .expect("compile-time evaluation")
        .value()
        .clone()
}

#[test]
fn typed_constants_parameters_operators_tuples_calls_and_conditionals_lower_exactly() {
    let (checked, program, eligible) = core_lowering_program();
    assert_control_flow_lowering(&checked, &program, &eligible);
    assert_constant_lowering(&checked, &program, &eligible);
}

fn core_lowering_program() -> (CheckedProgram, CompileTimeProgram, BTreeSet<FunctionId>) {
    let checked = check_program(
        "namespace Example\n\
         private function increment(value: Int8): Int8\n\
             return value + 1\n\
         end\n\
         public function choose(flag: Boolean, value: Int8): Int8\n\
             if flag then\n\
                 return increment(value)\n\
             else\n\
                 return -value\n\
             end\n\
         end\n\
         public function constants(): (Int, String, Boolean)\n\
             return (42, \"Pop\", not false)\n\
         end\n\
         public function maximum(): UInt64\n\
             return 18446744073709551615\n\
         end\n\
         public function half(value: Float32): Float32\n\
             return -value / 2\n\
         end\n\
         public function tupleEqual(value: Int): Boolean\n\
             return (value, true) == (42, true)\n\
         end\n\
         public function direct(): Int8\n\
             return increment(41)\n\
         end\n",
    );
    let lowered: Vec<_> = [
        "increment",
        "choose",
        "constants",
        "maximum",
        "half",
        "tupleEqual",
        "direct",
    ]
    .map(|name| checked.lower(name))
    .into_iter()
    .collect();
    let program = CompileTimeProgram::new(lowered, &checked.arena).expect("verified program");
    let eligible: BTreeSet<_> = program
        .functions()
        .iter()
        .map(pop_compile_time::CompileTimeFunction::function)
        .collect();
    (checked, program, eligible)
}

fn assert_control_flow_lowering(
    checked: &CheckedProgram,
    program: &CompileTimeProgram,
    eligible: &BTreeSet<FunctionId>,
) {
    let choose = function_id(&checked.functions["choose"]);
    assert_eq!(
        evaluate(
            program,
            eligible,
            choose,
            &[
                CompileTimeValue::Boolean(true),
                CompileTimeValue::Integer(
                    IntegerValue::parse_decimal("41", IntegerKind::Int8).expect("Int8"),
                ),
            ],
        ),
        CompileTimeValue::Integer(
            IntegerValue::parse_decimal("42", IntegerKind::Int8).expect("Int8"),
        )
    );
    assert_eq!(
        evaluate(
            program,
            eligible,
            function_id(&checked.functions["tupleEqual"]),
            &[CompileTimeValue::Integer(
                IntegerValue::parse_decimal("42", IntegerKind::Int64).expect("Int"),
            )],
        ),
        CompileTimeValue::Boolean(true)
    );

    let TypedStatementKind::Return { values } =
        checked.functions["direct"].body.statements()[0].kind()
    else {
        panic!("direct return");
    };
    let expression = lower_compile_time_expression(&values[0], &checked.arena)
        .expect("required constant expression");
    assert!(matches!(
        expression.kind(),
        pop_compile_time::CompileTimeExpressionKind::Call { .. }
    ));
    assert_eq!(
        evaluate(
            program,
            eligible,
            choose,
            &[
                CompileTimeValue::Boolean(false),
                CompileTimeValue::Integer(
                    IntegerValue::parse_decimal("2", IntegerKind::Int8).expect("Int8"),
                ),
            ],
        ),
        CompileTimeValue::Integer(
            IntegerValue::parse_decimal("-2", IntegerKind::Int8).expect("Int8"),
        )
    );
}

fn assert_constant_lowering(
    checked: &CheckedProgram,
    program: &CompileTimeProgram,
    eligible: &BTreeSet<FunctionId>,
) {
    assert_eq!(
        evaluate(
            program,
            eligible,
            function_id(&checked.functions["constants"]),
            &[],
        ),
        CompileTimeValue::Tuple(vec![
            CompileTimeValue::Integer(
                IntegerValue::parse_decimal("42", IntegerKind::Int64).expect("Int"),
            ),
            CompileTimeValue::String("Pop".to_owned()),
            CompileTimeValue::Boolean(true),
        ])
    );
    assert_eq!(
        evaluate(
            program,
            eligible,
            function_id(&checked.functions["maximum"]),
            &[],
        ),
        CompileTimeValue::Integer(
            IntegerValue::parse_decimal("18446744073709551615", IntegerKind::UInt64)
                .expect("UInt64"),
        )
    );
    assert_eq!(
        evaluate(
            program,
            eligible,
            function_id(&checked.functions["half"]),
            &[CompileTimeValue::Float(
                FloatValue::parse_decimal("4", FloatKind::Float32).expect("Float32"),
            )],
        ),
        CompileTimeValue::Float(
            FloatValue::parse_decimal("-2", FloatKind::Float32).expect("Float32"),
        )
    );
}

#[test]
fn lowered_boolean_operators_are_short_circuiting() {
    let checked = check_program(
        "namespace Example\n\
         private function forbidden(): Boolean\n\
             return 1 / 0 > 0\n\
         end\n\
         public function safeAnd(): Boolean\n\
             return false and forbidden()\n\
         end\n\
         public function safeOr(): Boolean\n\
             return true or forbidden()\n\
         end\n",
    );
    let program = CompileTimeProgram::new(
        vec![
            checked.lower("forbidden"),
            checked.lower("safeAnd"),
            checked.lower("safeOr"),
        ],
        &checked.arena,
    )
    .expect("verified program");
    let safe_and = function_id(&checked.functions["safeAnd"]);
    let safe_or = function_id(&checked.functions["safeOr"]);
    let eligible = BTreeSet::from([safe_and, safe_or]);

    assert_eq!(
        evaluate(&program, &eligible, safe_and, &[]),
        CompileTimeValue::Boolean(false)
    );
    assert_eq!(
        evaluate(&program, &eligible, safe_or, &[]),
        CompileTimeValue::Boolean(true)
    );
}

#[test]
fn restricted_lowering_rejects_state_loops_mutation_and_non_direct_dispatch() {
    let checked = check_program(
        "namespace Example\n\
         public class Counter\n\
             public value: Int\n\
             public function Counter:get(): Int\n\
                 return self.value\n\
             end\n\
         end\n\
         public function loop(): Int\n\
             while false do\n\
             end\n\
             return 1\n\
         end\n\
         public function mutation(counter: Counter): Int\n\
             counter.value = 1\n\
             return 1\n\
         end\n\
         public function method(counter: Counter): Int\n\
             return counter:get()\n\
         end\n\
         public function indirect(callback: function(value: Int): Int): Int\n\
             return callback(1)\n\
         end\n",
    );

    for (name, expected) in [
        ("loop", UnsupportedCompileTimeConstruct::Loop),
        ("mutation", UnsupportedCompileTimeConstruct::Mutation),
        ("method", UnsupportedCompileTimeConstruct::MethodCall),
        ("indirect", UnsupportedCompileTimeConstruct::IndirectCall),
    ] {
        assert!(
            matches!(
                checked.lower_error(name),
                CompileTimeLoweringError::UnsupportedConstruct { construct, .. }
                    if construct == expected
            ),
            "{name}"
        );
    }
}

#[test]
fn immutable_locals_lower_with_lexical_identity_and_evaluate_deterministically() {
    let checked = check_program(
        "namespace Example\n\
         public function calculate(value: Int): Int\n\
             local incremented = value + 1\n\
             local doubled: Int = incremented * 2\n\
             return doubled\n\
         end\n",
    );
    let function = checked.lower("calculate");
    let id = function.function();
    let program = CompileTimeProgram::new(vec![function], &checked.arena).expect("program");
    let eligible = BTreeSet::from([id]);

    assert_eq!(
        evaluate(
            &program,
            &eligible,
            id,
            &[CompileTimeValue::Integer(
                IntegerValue::parse_decimal("20", IntegerKind::Int64).expect("Int"),
            )],
        ),
        CompileTimeValue::Integer(
            IntegerValue::parse_decimal("42", IntegerKind::Int64).expect("Int"),
        )
    );
}

#[test]
fn restricted_lowering_rejects_result_packs_and_non_deterministic_body_shapes() {
    let checked = check_program(
        "namespace Example\n\
         public function noResult()\n\
             return\n\
         end\n\
         public function manyResults(): (Int, Int)\n\
             return (1, 2)\n\
         end\n\
         public function trailingStatements(): Int\n\
             1\n\
             return 2\n\
         end\n",
    );

    assert!(matches!(
        checked.lower_error("noResult"),
        CompileTimeLoweringError::UnsupportedResultArity { found: 0 }
    ));
    // A tuple is one result; this source confirms tuples stay supported rather than
    // being confused with Pop Lang's statically typed multiple-result packs.
    assert_eq!(
        checked.lower("manyResults").body().type_id(),
        checked.functions["manyResults"].signature.results()[0]
            .type_id()
            .expect("tuple result")
    );
    assert!(matches!(
        checked.lower_error("trailingStatements"),
        CompileTimeLoweringError::BodyDoesNotProduceSingleResult { .. }
    ));
}
