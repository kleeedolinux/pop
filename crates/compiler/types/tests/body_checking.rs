use std::collections::BTreeMap;
use std::fmt::Write as _;

use pop_foundation::{BubbleId, DiagnosticArgument, FileId, ModuleId, SymbolId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_file, parse_function_body, parse_function_signature};
use pop_types::{
    BodyChecker, CaptureMode, FloatKind, NumericConversionKind, SemanticType, SignatureResolver,
    StringFormatKind, TypedBinaryOperator, TypedCompoundOperator, TypedExpressionKind,
    TypedStatementKind, embedded_bootstrap_schema,
};

struct CheckedFixture {
    result: pop_types::TypedBodyResult,
    arena: pop_types::TypeArena,
    symbols: BTreeMap<String, SymbolId>,
}

fn check_function(source_text: &str, checked_name: &str) -> CheckedFixture {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/body.pop", source_text).expect("source");
    let syntax = parse_file(&source);
    assert!(
        syntax.diagnostics().is_empty(),
        "structural syntax: {}",
        syntax.diagnostic_snapshot()
    );
    let parsed: Vec<_> = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::FunctionDeclaration)
        .map(|node| {
            let signature =
                parse_function_signature(&source, &syntax, node).expect("function signature");
            let body = parse_function_body(&source, &syntax, node, &signature).expect("body");
            (node, signature, body)
        })
        .collect();
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let mut symbols = BTreeMap::new();
    for (_, signature, _) in &parsed {
        let qualified = format!("Example.{}", signature.name());
        let symbol = indexed
            .index()
            .declaration_by_qualified_name(&qualified, SymbolSpace::Value)[0]
            .symbol();
        symbols.insert(signature.name().to_owned(), symbol);
    }
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let mut signatures = BTreeMap::new();
    for (_, syntax_signature, _) in &parsed {
        let symbol = symbols[syntax_signature.name()];
        let resolution_result = resolver.resolve(module, symbol, syntax_signature);
        assert!(
            resolution_result.diagnostics().is_empty(),
            "resolved signature"
        );
        signatures.insert(
            symbol,
            resolution_result
                .signature()
                .expect("valid signature")
                .clone(),
        );
    }
    let (_, _, body) = parsed
        .iter()
        .find(|(_, signature, _)| signature.name() == checked_name)
        .expect("checked function");
    let checked_symbol = symbols[checked_name];
    let result = BodyChecker::new(module, &mut resolver, &signatures)
        .check(&signatures[&checked_symbol], body);
    CheckedFixture {
        result,
        arena: resolver.into_arena(),
        symbols,
    }
}

#[test]
fn infers_locals_and_resolves_direct_calls_with_canonical_types() {
    let fixture = check_function(
        "namespace Example\n\
         private function combine(left: Int, right: Int): Int\n\
             return left + right\n\
         end\n\
         public function calculate(left: Int, right: Int): Int\n\
             local sum: Int = left + right\n\
             local copy = sum\n\
             return combine(copy, right)\n\
         end\n",
        "calculate",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let int = fixture.arena.source_type("Int").expect("Int");
    let TypedStatementKind::Local {
        local,
        local_type,
        initializer,
        ..
    } = body.statements()[0].kind()
    else {
        panic!("typed local");
    };
    assert_eq!(*local_type, int);
    assert_eq!(local.raw(), 0);
    assert_eq!(initializer.type_id(), int);
    assert!(matches!(
        fixture.arena.get(int),
        Some(SemanticType::Primitive(_))
    ));

    let TypedStatementKind::Return { values } = body.statements()[2].kind() else {
        panic!("typed return");
    };
    assert!(matches!(
        values[0].kind(),
        TypedExpressionKind::DirectCall { function, .. }
            if *function == fixture.symbols["combine"]
    ));
    assert_eq!(values[0].type_id(), int);
}

#[test]
fn reports_annotation_and_return_type_mismatches_without_dynamic_fallback() {
    for (source, expected_code) in [
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 local value: Boolean = 1\n\
                 return 1\n\
             end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
             public function invalid(): Boolean\n\
                 return 1\n\
             end\n",
            "POP2003",
        ),
    ] {
        let fixture = check_function(source, "invalid");
        assert!(fixture.result.body().is_none());
        assert!(
            fixture
                .result
                .diagnostic_snapshot()
                .starts_with(expected_code)
        );
    }
}

#[test]
fn omitted_return_annotation_is_an_empty_result_pack_not_inference() {
    let empty = check_function(
        "namespace Example\n\
         internal function log(value: Int)\n\
             return\n\
         end\n",
        "log",
    );
    assert!(empty.result.diagnostics().is_empty());

    let valued = check_function(
        "namespace Example\n\
         internal function add(left: Int, right: Int)\n\
             return left + right\n\
         end\n",
        "add",
    );
    assert!(valued.result.body().is_none());
    assert!(valued.result.diagnostic_snapshot().starts_with("POP2004"));
}

#[test]
fn checks_fixed_pack_returns_declarations_swaps_and_call_destructuring() {
    let fixture = check_function(
        "namespace Example\n\
         private function split(value: Int): (Int, String)\n\
             return value, String(value)\n\
         end\n\
         public function exchange(value: Int): (Int, String)\n\
             local number: Int, text: String = split(value)\n\
             local nextNumber = number + 1\n\
             number, nextNumber = nextNumber, number\n\
             return number, text\n\
         end\n",
        "exchange",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
}

#[test]
fn rejects_fixed_pack_arity_type_and_scalar_context_mismatches() {
    for source in [
        "namespace Example\npublic function invalid(): (Int, String)\nreturn 1\nend\n",
        "namespace Example\npublic function invalid(): (Int, String)\nreturn 1, 2\nend\n",
        "namespace Example\npublic function invalid(): Int\nlocal left, right = 1\nreturn left\nend\n",
        "namespace Example\npublic function invalid(): Int\nlocal value, value = 1, 2\nreturn value\nend\n",
    ] {
        let fixture = check_function(source, "invalid");
        assert!(
            !fixture.result.diagnostics().is_empty(),
            "fixed packs require exact static arity and types: {source}"
        );
    }
}

#[test]
fn fixed_pack_arity_diagnostic_reports_the_exact_target_count() {
    let fixture = check_function(
        "namespace Example\n\
         public function invalid(): Int\n\
             local first, second, third = 1\n\
             return first\n\
         end\n",
        "invalid",
    );
    let diagnostic = fixture
        .result
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code().as_str() == "POP2004")
        .expect("fixed-pack arity diagnostic");

    assert_eq!(
        diagnostic.arguments(),
        [
            DiagnosticArgument::Identifier("multiple local".to_owned()),
            DiagnosticArgument::Unsigned(3),
            DiagnosticArgument::Unsigned(1),
        ]
    );
}

#[test]
fn tuple_projection_is_one_based_static_and_exactly_typed() {
    let fixture = check_function(
        "namespace Example\n\
         private function pair(value: Int): (Int, String)\n\
             return value, String(value)\n\
         end\n\
         public function select(value: Int): String\n\
             local result = pair(value)\n\
             return result[2]\n\
         end\n",
        "select",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
}

#[test]
fn tuple_projection_rejects_dynamic_and_out_of_range_indexes() {
    for expression in ["result[index]", "result[-1]", "result[0]", "result[3]"] {
        let source = format!(
            "namespace Example\n\
             public function invalid(index: Int): Int\n\
                 local result = (1, 2)\n\
                 return {expression}\n\
             end\n"
        );
        let fixture = check_function(&source, "invalid");
        assert!(
            !fixture.result.diagnostics().is_empty(),
            "tuple projection accepted {expression}"
        );
    }
}

#[test]
fn reports_unknown_values_wrong_call_arity_and_invalid_operands() {
    for (source, expected_code) in [
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 return missing\n\
             end\n",
            "POP1002",
        ),
        (
            "namespace Example\n\
             private function identity(value: Int): Int\n\
                 return value\n\
             end\n\
             public function invalid(): Int\n\
                 return identity(1, 2)\n\
             end\n",
            "POP2004",
        ),
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 return true + 1\n\
             end\n",
            "POP2005",
        ),
    ] {
        let fixture = check_function(source, "invalid");
        assert!(fixture.result.body().is_none());
        assert!(
            fixture
                .result
                .diagnostic_snapshot()
                .starts_with(expected_code)
        );
    }
}

#[test]
fn types_decimal_literals_ordering_and_explicit_numeric_conversions() {
    // ADR 0040: an unhinted decimal is Float64, an annotation may select
    // Float32, and target-type calls become explicit typed conversions.
    let fixture = check_function(
        "namespace Example\n\
         public function convert(count: Int): Boolean\n\
             local defaultRatio = 1.5\n\
             local compactRatio: Float32 = 1.5\n\
             local converted = Float64(count)\n\
             local narrowed = Int32(converted)\n\
             return converted <= defaultRatio and converted >= Float64(compactRatio)\n\
         end\n",
        "convert",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let float64 = fixture.arena.source_type("Float64").expect("Float64");
    let float32 = fixture.arena.source_type("Float32").expect("Float32");
    let local_initializer = |index: usize| -> &pop_types::TypedExpression {
        let TypedStatementKind::Local { initializer, .. } = body.statements()[index].kind() else {
            panic!("local initializer");
        };
        initializer
    };
    assert_eq!(local_initializer(0).type_id(), float64);
    assert_eq!(local_initializer(1).type_id(), float32);
    assert!(matches!(
        local_initializer(2).kind(),
        TypedExpressionKind::NumericConvert {
            conversion: NumericConversionKind::IntegerToFloat {
                target: FloatKind::Float64,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        local_initializer(3).kind(),
        TypedExpressionKind::NumericConvert {
            conversion: NumericConversionKind::FloatToInteger { .. },
            ..
        }
    ));
    let TypedStatementKind::Return { values } = body.statements()[4].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        TypedExpressionKind::Binary {
            operator: TypedBinaryOperator::And,
            left,
            right,
        } if matches!(left.kind(), TypedExpressionKind::Binary {
            operator: TypedBinaryOperator::LessThanOrEqual,
            ..
        }) && matches!(right.kind(), TypedExpressionKind::Binary {
            operator: TypedBinaryOperator::GreaterThanOrEqual,
            ..
        })
    ));
}

#[test]
fn types_string_composition_and_closed_primitive_formatting() {
    // ADR 0041: interpolation and String(value) are closed static operations,
    // never universal formatting or dynamic dispatch.
    let fixture = check_function(
        "namespace Example\n\
         public function describe(count: Int, enabled: Boolean): String\n\
             local explicit = String(count)\n\
             local text = `count={count}, enabled={enabled}`\n\
             return explicit .. \"; \" .. text\n\
         end\n",
        "describe",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("explicit format local");
    };
    assert!(matches!(
        initializer.kind(),
        TypedExpressionKind::StringFormat {
            kind: StringFormatKind::Integer(_),
            ..
        }
    ));
    let TypedStatementKind::Return { values } = body.statements()[2].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        TypedExpressionKind::StringConcat { .. }
    ));
}

#[test]
fn rejects_non_string_concat_and_non_primitive_formatting() {
    for source in [
        "namespace Example\n\
         public function invalid(): String\n\
             return \"count=\" .. 1\n\
         end\n",
        "namespace Example\n\
         public function invalid(values: {Int}): String\n\
             return String(values)\n\
         end\n",
        "namespace Example\n\
         public function invalid(values: {[String]: Int}): String\n\
             return `values={values}`\n\
         end\n",
    ] {
        let fixture = check_function(source, "invalid");
        assert!(fixture.result.body().is_none());
        assert!(
            fixture.result.diagnostic_snapshot().starts_with("POP2005"),
            "{}\n{source}",
            fixture.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn rejects_decimal_integer_targets_and_nonnumeric_cast_arguments() {
    for (source, expected_code) in [
        (
            "namespace Example\n\
         public function invalid(): Int\n\
             local value: Int = 1.5\n\
             return value\n\
         end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
         public function invalid(): Int\n\
             return Int(\"1\")\n\
         end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
         public function invalid(): Int\n\
             return Int(1, 2)\n\
         end\n",
            "POP2004",
        ),
    ] {
        let fixture = check_function(source, "invalid");
        assert!(fixture.result.body().is_none());
        assert_eq!(fixture.result.diagnostics().len(), 1);
        assert_eq!(
            fixture.result.diagnostics()[0].code().as_str(),
            expected_code
        );
    }
}

#[test]
fn types_structured_control_flow_and_proves_returns_on_both_branches() {
    let fixture = check_function(
        "namespace Example\n\
         public function choose(condition: Boolean): Int\n\
             while condition do\n\
                 condition\n\
             end\n\
             if condition then\n\
                 return 1\n\
             else\n\
                 return 2\n\
             end\n\
         end\n",
        "choose",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    assert!(matches!(
        body.statements()[0].kind(),
        TypedStatementKind::While { body, .. } if body.len() == 1
    ));
    assert!(matches!(
        body.statements()[1].kind(),
        TypedStatementKind::If {
            then_body,
            else_body,
            ..
        } if then_body.len() == 1 && else_body.len() == 1
    ));
}

#[test]
fn types_optional_binding_defaulting_propagation_and_nil_narrowing() {
    // ADR 0051: optional control is presence-based and keeps the inner type in
    // typed bodies; postfix propagation only requires an optional function
    // result and does not relate the operand/result inner types.
    let fixture = check_function(
        "namespace Example\n\
         private function choose(value: String?, enabled: Boolean?, fallback: String): Boolean?\n\
             local selected = value ?? fallback\n\
             local present = value?\n\
             if value ~= nil then\n\
                 useString(value)\n\
             end\n\
             if local bound = enabled then\n\
                 useBoolean(bound)\n\
             else\n\
                 useString(selected)\n\
             end\n\
             while local bound = enabled do\n\
                 useBoolean(bound)\n\
                 break\n\
             end\n\
             return enabled\n\
         end\n\
         private function useString(value: String)\n\
         end\n\
         private function useBoolean(value: Boolean)\n\
         end\n",
        "choose",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed optional body");
    let string = fixture.arena.source_type("String").expect("String");
    let boolean = fixture.arena.source_type("Boolean").expect("Boolean");
    assert!(matches!(
        body.statements()[0].kind(),
        TypedStatementKind::Local { initializer, local_type, .. }
            if *local_type == string
                && matches!(initializer.kind(), TypedExpressionKind::OptionalDefault { .. })
    ));
    assert!(matches!(
        body.statements()[1].kind(),
        TypedStatementKind::Local { initializer, local_type, .. }
            if *local_type == string
                && matches!(initializer.kind(), TypedExpressionKind::OptionalPropagate { .. })
    ));
    assert!(matches!(
        body.statements()[3].kind(),
        TypedStatementKind::OptionalIf {
            name,
            inner_type,
            then_body,
            else_body,
            ..
        } if name == "bound"
            && *inner_type == boolean
            && then_body.len() == 1
            && else_body.len() == 1
    ));
    assert!(matches!(
        body.statements()[4].kind(),
        TypedStatementKind::OptionalWhile {
            name,
            inner_type,
            body,
            ..
        } if name == "bound" && *inner_type == boolean && body.len() == 2
    ));
}

#[test]
fn rejects_invalid_optional_control_without_dynamic_fallback() {
    for source in [
        "namespace Example\n\
         private function invalid(): Int\n\
             return 1 ?? 2\n\
         end\n",
        "namespace Example\n\
         private function invalid(): Int?\n\
             local value = 1?\n\
             return value\n\
         end\n",
        "namespace Example\n\
         private function invalid(value: Int?): Int\n\
             local present = value?\n\
             return present\n\
         end\n",
        "namespace Example\n\
         private function invalid(value: Int?): Int\n\
             if local present = value then\n\
                 return present\n\
             else\n\
                 return present\n\
             end\n\
         end\n",
        "namespace Example\n\
         private function useString(value: String)\n\
         end\n\
         private function invalid(value: String?, replacement: String?)\n\
             local current = value\n\
             if current ~= nil then\n\
                 current = replacement\n\
                 useString(current)\n\
             end\n\
         end\n",
        "namespace Example\n\
         private function useString(value: String)\n\
         end\n\
         private function invalid(value: String?, replacement: String?)\n\
             local current = value\n\
             local function clear()\n\
                 current = replacement\n\
             end\n\
             if current ~= nil then\n\
                 clear()\n\
                 useString(current)\n\
             end\n\
         end\n",
    ] {
        let fixture = check_function(source, "invalid");
        assert!(fixture.result.body().is_none(), "accepted:\n{source}");
        assert!(!fixture.result.diagnostics().is_empty(), "{source}");
    }
}

#[test]
fn repeat_until_requires_boolean_conditions_and_keeps_body_locals_scoped_to_the_loop() {
    // ADR 0060: the body scope includes its corresponding `until` condition,
    // but does not escape the completed repeat-until statement.
    let accepted = check_function(
        "namespace Example\n\
         public function count(): Int\n\
             local value = 0\n\
             repeat\n\
                 local limit = 3\n\
                 value = value + 1\n\
             until value == limit\n\
             return value\n\
         end\n",
        "count",
    );
    assert!(
        accepted.result.diagnostics().is_empty(),
        "{}",
        accepted.result.diagnostic_snapshot()
    );
    assert_eq!(
        accepted
            .result
            .body()
            .expect("typed repeat-until body")
            .statements()
            .len(),
        3
    );

    for (source, expected_code) in [
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 repeat\n\
                     local value = 1\n\
                 until value\n\
                 return 0\n\
             end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 repeat\n\
                     local limit = 1\n\
                 until limit == 1\n\
                 return limit\n\
             end\n",
            "POP1002",
        ),
    ] {
        let rejected = check_function(source, "invalid");
        assert!(rejected.result.body().is_none());
        assert!(
            rejected
                .result
                .diagnostic_snapshot()
                .starts_with(expected_code),
            "{}",
            rejected.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn numeric_for_ranges_and_loop_control_are_closed_static_statements() {
    // ADR 0042: range values share one integer type and loop control resolves
    // lexically without a runtime iterator lookup.
    let accepted = check_function(
        "namespace Example\n\
         public function count(limit: Int): Int\n\
             local total = 0\n\
             for index = 1, limit do\n\
                 if index == 2 then\n\
                     continue\n\
                 end\n\
                 total = total + index\n\
                 if total > 20 then\n\
                     break\n\
                 end\n\
             end\n\
             return total\n\
         end\n",
        "count",
    );
    assert!(
        accepted.result.diagnostics().is_empty(),
        "{}",
        accepted.result.diagnostic_snapshot()
    );
    let statements = accepted.result.body().expect("typed body").statements();
    assert!(matches!(
        statements[1].kind(),
        TypedStatementKind::NumericFor { body, .. }
            if matches!(body[0].kind(), TypedStatementKind::If { then_body, .. }
                if matches!(then_body[0].kind(), TypedStatementKind::Continue))
                && matches!(body[2].kind(), TypedStatementKind::If { then_body, .. }
                    if matches!(then_body[0].kind(), TypedStatementKind::Break))
    ));
}

#[test]
fn numeric_for_rejects_dynamic_boundaries_and_invalid_loop_control() {
    for (source, expected) in [
        (
            "namespace Example\n\
         public function invalid(limit: Float64)\n\
             for index = 1, limit do\n\
                 index\n\
             end\n\
         end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
         public function invalid()\n\
             for index = 1, 3 do\n\
                 index = 2\n\
             end\n\
         end\n",
            "POP2005",
        ),
        (
            "namespace Example\n\
         public function invalid()\n\
             for index = 1, 3, 0 do\n\
                 index\n\
             end\n\
         end\n",
            "POP2005",
        ),
        (
            "namespace Example\n\
         public function invalid()\n\
             break\n\
         end\n",
            "POP2005",
        ),
        (
            "namespace Example\n\
         public function invalid()\n\
             repeat\n\
                 continue\n\
                 local value = 1\n\
             until value == 1\n\
         end\n",
            "POP2005",
        ),
        (
            "namespace Example\n\
         public function invalid()\n\
             while true do\n\
                 local escape = function()\n\
                     continue\n\
                 end\n\
                 escape()\n\
             end\n\
         end\n",
            "POP2005",
        ),
    ] {
        let rejected = check_function(source, "invalid");
        assert!(rejected.result.body().is_none());
        assert!(
            rejected.result.diagnostic_snapshot().contains(expected),
            "{}",
            rejected.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn rejects_non_boolean_conditions_and_missing_return_paths() {
    for (source, expected_code) in [
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 if 1 then\n\
                     return 1\n\
                 else\n\
                     return 2\n\
                 end\n\
             end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
             public function invalid(condition: Boolean): Int\n\
                 if condition then\n\
                     return 1\n\
                 end\n\
             end\n",
            "POP2006",
        ),
    ] {
        let fixture = check_function(source, "invalid");
        assert!(fixture.result.body().is_none());
        assert!(
            fixture
                .result
                .diagnostic_snapshot()
                .starts_with(expected_code)
        );
    }
}

#[test]
fn conditional_expressions_require_boolean_conditions_and_one_static_type() {
    let accepted = check_function(
        "namespace Example\n\
         public function choose(condition: Boolean): Int8\n\
             return if condition then 1 else 2\n\
         end\n",
        "choose",
    );
    assert!(
        accepted.result.diagnostics().is_empty(),
        "{}",
        accepted.result.diagnostic_snapshot()
    );
    let [statement] = accepted.result.body().expect("typed body").statements() else {
        panic!("one return");
    };
    assert!(matches!(
        statement.kind(),
        TypedStatementKind::Return { values }
            if matches!(values[0].kind(), TypedExpressionKind::Conditional { .. })
                && matches!(
                    accepted.arena.get(values[0].type_id()),
                    Some(SemanticType::Primitive(pop_types::PrimitiveType::Integer(
                        pop_types::IntegerKind::Int8
                    )))
                )
    ));

    for source in [
        "namespace Example\n\
         public function invalid(): Int\n\
             return if 1 then 2 else 3\n\
         end\n",
        "namespace Example\n\
         public function invalid(condition: Boolean): Int\n\
             return if condition then 1 else \"wrong\"\n\
         end\n",
    ] {
        let rejected = check_function(source, "invalid");
        assert!(rejected.result.body().is_none());
        assert!(
            rejected.result.diagnostic_snapshot().contains("POP2003"),
            "{}",
            rejected.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn compound_assignment_preserves_exact_types_targets_and_operators() {
    let accepted = check_function(
        "namespace Example\n\
         public function update(values: {Int}): Int8\n\
             local total: Int8 = 1\n\
             local message = \"\"\n\
             total += 2\n\
             values[1] += 4\n\
             message ..= \"!\"\n\
             return total\n\
         end\n",
        "update",
    );
    assert!(
        accepted.result.diagnostics().is_empty(),
        "{}",
        accepted.result.diagnostic_snapshot()
    );
    let statements = accepted.result.body().expect("typed body").statements();
    assert!(matches!(
        statements[2].kind(),
        TypedStatementKind::LocalSet { value, .. }
            if matches!(value.kind(), TypedExpressionKind::Binary {
                operator: TypedBinaryOperator::Add,
                ..
            })
    ));
    assert!(matches!(
        statements[3].kind(),
        TypedStatementKind::CompoundArraySet {
            operator: TypedCompoundOperator::Add,
            ..
        }
    ));
    assert!(matches!(
        statements[4].kind(),
        TypedStatementKind::LocalSet { value, .. }
            if matches!(value.kind(), TypedExpressionKind::StringConcat { .. })
    ));

    for source in [
        "namespace Example\n\
         public function invalid(value: Float64): Float64\n\
             local result = value\n\
             result %= 2.0\n\
             return result\n\
         end\n",
        "namespace Example\n\
         public function invalid(value: Int): Int\n\
             local result = value\n\
             result += 1.5\n\
             return result\n\
         end\n",
        "namespace Example\n\
         public function invalid(value: Int): Int\n\
             value += 1\n\
             return value\n\
         end\n",
    ] {
        let rejected = check_function(source, "invalid");
        assert!(rejected.result.body().is_none());
        assert!(
            rejected.result.diagnostic_snapshot().contains("POP2005")
                || rejected.result.diagnostic_snapshot().contains("POP2003"),
            "{}",
            rejected.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn compound_arithmetic_accepts_every_exact_numeric_kind() {
    for type_name in [
        "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float32",
        "Float64",
    ] {
        let mut operations = String::from(
            "    local result = value\n    result += 1\n    result -= 1\n    result *= 2\n    result /= 2\n",
        );
        if !type_name.starts_with("Float") {
            operations.push_str("    result %= 3\n");
        }
        let mut source = String::new();
        write!(
            source,
            "namespace Example\npublic function update(value: {type_name}): {type_name}\n{operations}    return result\nend\n"
        )
        .expect("source text");
        let accepted = check_function(&source, "update");
        assert!(
            accepted.result.diagnostics().is_empty(),
            "{type_name}: {}",
            accepted.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn branch_locals_do_not_escape_their_lexical_scope() {
    let fixture = check_function(
        "namespace Example\n\
         public function invalid(condition: Boolean): Int\n\
             if condition then\n\
                 local branchValue = 1\n\
             end\n\
             return branchValue\n\
         end\n",
        "invalid",
    );

    assert!(fixture.result.body().is_none());
    assert!(fixture.result.diagnostic_snapshot().starts_with("POP1002"));
}

#[test]
fn local_and_anonymous_closures_capture_lexical_values_with_static_function_types() {
    let fixture = check_function(
        "namespace Example\n\
         public function make(offset: Int): function(value: Int): Int\n\
             local function add(value: Int): Int\n\
                 return value + offset\n\
             end\n\
             local copy = function(value: Int): Int\n\
                 return add(value)\n\
             end\n\
             return copy\n\
         end\n",
        "make",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed closure body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("local function lowered to a typed closure binding");
    };
    let TypedExpressionKind::Closure(closure) = initializer.kind() else {
        panic!("closure expression");
    };
    assert_eq!(closure.captures().len(), 1);
    assert_eq!(closure.captures()[0].mode(), CaptureMode::Value);
    assert!(matches!(
        fixture.arena.get(initializer.type_id()),
        Some(SemanticType::Function { parameters, results, .. })
            if parameters.len() == 1 && results.len() == 1
    ));

    let TypedStatementKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("anonymous closure local");
    };
    let TypedExpressionKind::Closure(closure) = initializer.kind() else {
        panic!("anonymous closure");
    };
    assert_eq!(closure.captures().len(), 1);
}

#[test]
fn async_calls_return_tasks_and_await_extracts_the_completion_type() {
    let fixture = check_function(
        "namespace Example\n\
         public async function load(): Int\n\
             return 42\n\
         end\n\
         public async function useTask(): Int\n\
             local pending = load()\n\
             return await pending\n\
         end\n",
        "useTask",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("pending local");
    };
    let Some(SemanticType::Builtin {
        definition,
        arguments,
    }) = fixture.arena.get(initializer.type_id())
    else {
        panic!("task type");
    };
    assert_eq!(arguments.len(), 1);
    assert_eq!(
        fixture.arena.get(*arguments.first().expect("completion")),
        fixture
            .arena
            .get(fixture.arena.source_type("Int").expect("Int"))
    );
    let task = fixture
        .arena
        .find(&SemanticType::Builtin {
            definition: *definition,
            arguments: arguments.clone(),
        })
        .expect("canonical task");
    assert_eq!(initializer.type_id(), task);

    let TypedStatementKind::Return { values } = body.statements()[1].kind() else {
        panic!("return");
    };
    let TypedExpressionKind::Await { task } = values[0].kind() else {
        panic!("await expression");
    };
    assert_eq!(task.type_id(), initializer.type_id());
    assert_eq!(
        values[0].type_id(),
        fixture.arena.source_type("Int").expect("Int")
    );
}

#[test]
fn await_rejects_non_task_operands_without_dynamic_fallback() {
    let fixture = check_function(
        "namespace Example\n\
         public async function invalid(value: Int): Int\n\
             return await value\n\
         end\n",
        "invalid",
    );

    assert!(fixture.result.body().is_none());
    assert!(fixture.result.diagnostic_snapshot().contains("POP2005"));
}

#[test]
fn await_is_valid_only_inside_async_bodies() {
    let fixture = check_function(
        "namespace Example\n\
         public async function load(): Int\n\
             return 42\n\
         end\n\
         public function invalid(): Int\n\
             local pending = load()\n\
             return await pending\n\
         end\n",
        "invalid",
    );

    assert!(fixture.result.body().is_none());
    assert!(fixture.result.diagnostic_snapshot().contains("POP2005"));
}

#[test]
fn async_closure_bodies_keep_async_context_for_await() {
    let fixture = check_function(
        "namespace Example\n\
         public async function load(): Int\n\
             return 42\n\
         end\n\
         public async function run(): Int\n\
             local worker = async function(): Int\n\
                 return await load()\n\
             end\n\
             return await worker()\n\
         end\n",
        "run",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
}

#[test]
fn async_defer_is_valid_only_inside_async_bodies() {
    let accepted = check_function(
        "namespace Example\n\
         public async function close(): Int\n\
             return 0\n\
         end\n\
         public async function valid(): Int\n\
             async defer\n\
                 local ignored = await close()\n\
             end\n\
             return 1\n\
         end\n",
        "valid",
    );

    assert!(
        accepted.result.diagnostics().is_empty(),
        "{}",
        accepted.result.diagnostic_snapshot()
    );
    let body = accepted.result.body().expect("typed body");
    assert!(matches!(
        body.statements()[0].kind(),
        TypedStatementKind::AsyncDefer { .. }
    ));

    let rejected = check_function(
        "namespace Example\n\
         public function invalid(): Int\n\
             async defer\n\
                 local value = 1\n\
             end\n\
             return 1\n\
         end\n",
        "invalid",
    );

    assert!(rejected.result.body().is_none());
    assert!(rejected.result.diagnostic_snapshot().contains("POP2005"));
}

#[test]
fn captured_mutation_uses_one_typed_cell_and_shadowing_does_not_capture() {
    let fixture = check_function(
        "namespace Example\n\
         public function make(): function(delta: Int): Int\n\
             local total = 0\n\
             local function add(delta: Int): Int\n\
                 total = total + delta\n\
                 local shadow = total\n\
                 return shadow\n\
             end\n\
             return add\n\
         end\n",
        "make",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("local function");
    };
    let TypedExpressionKind::Closure(closure) = initializer.kind() else {
        panic!("closure");
    };
    assert_eq!(closure.captures().len(), 1);
    assert_eq!(closure.captures()[0].mode(), CaptureMode::Cell);
    assert!(matches!(
        closure.body().statements()[0].kind(),
        TypedStatementKind::CaptureSet { .. }
    ));
}

#[test]
fn recursive_local_function_and_nested_capture_propagation_are_resolved_by_identity() {
    let fixture = check_function(
        "namespace Example\n\
         public function make(seed: Int): function(value: Int): Int\n\
             local function outer(value: Int): Int\n\
                 local function inner(next: Int): Int\n\
                     if next > 0 then\n\
                         return outer(next - 1) + seed\n\
                     else\n\
                         return seed\n\
                     end\n\
                 end\n\
                 return inner(value)\n\
             end\n\
             return outer\n\
         end\n",
        "make",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("outer closure");
    };
    let TypedExpressionKind::Closure(outer) = initializer.kind() else {
        panic!("outer closure");
    };
    assert!(
        outer
            .captures()
            .iter()
            .any(|capture| capture.mode() == CaptureMode::Cell)
    );
    let TypedStatementKind::Local { initializer, .. } = outer.body().statements()[0].kind() else {
        panic!("inner closure");
    };
    let TypedExpressionKind::Closure(inner) = initializer.kind() else {
        panic!("inner closure");
    };
    assert!(
        inner.captures().len() >= 2,
        "recursive function and seed propagate"
    );
}

#[test]
fn local_assignment_never_changes_the_binding_type() {
    let fixture = check_function(
        "namespace Example\n\
         public function invalid(): Int\n\
             local value = 1\n\
             value = false\n\
             return value\n\
         end\n",
        "invalid",
    );
    assert!(fixture.result.body().is_none());
    assert!(fixture.result.diagnostic_snapshot().starts_with("POP2003"));
}

#[test]
fn fixed_width_literals_and_binary_context_keep_the_exact_declared_type() {
    for source in [
        "namespace Example\n\
         public function value(): Int8\n\
             return 127\n\
         end\n",
        "namespace Example\n\
         public function value(): Int8\n\
             return -128\n\
         end\n",
        "namespace Example\n\
         public function value(): UInt64\n\
             return 18446744073709551615\n\
         end\n",
        "namespace Example\n\
         public function add(value: Int8): Int8\n\
             return value + 1\n\
         end\n",
        "namespace Example\n\
         public function add(value: Int8): Int8\n\
             return 1 + value\n\
         end\n",
        "namespace Example\n\
         public function value(): Int8\n\
             local result: Int8 = 1 + 2\n\
             return result\n\
         end\n",
        "namespace Example\n\
         public function add(value: Float32): Float32\n\
             return value + 1\n\
         end\n",
    ] {
        let fixture = check_function(
            source,
            if source.contains("function add") {
                "add"
            } else {
                "value"
            },
        );
        assert!(
            fixture.result.diagnostics().is_empty(),
            "{}",
            fixture.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn numeric_typing_rejects_out_of_range_and_mixed_or_unsupported_operations() {
    for (source, expected) in [
        (
            "namespace Example\n\
             public function value(): Int8\n\
                 return 128\n\
             end\n",
            "POP2013",
        ),
        (
            "namespace Example\n\
             public function value(): UInt8\n\
                 return -1\n\
             end\n",
            "POP2013",
        ),
        (
            "namespace Example\n\
             public function add(left: Int8, right: Int16): Int8\n\
                 return left + right\n\
             end\n",
            "POP2005",
        ),
        (
            "namespace Example\n\
             public function remainder(left: Float32, right: Float32): Float32\n\
                 return left % right\n\
             end\n",
            "POP2005",
        ),
    ] {
        let name = if source.contains("function add") {
            "add"
        } else if source.contains("function remainder") {
            "remainder"
        } else {
            "value"
        };
        let fixture = check_function(source, name);
        assert!(fixture.result.body().is_none());
        assert!(
            fixture.result.diagnostic_snapshot().starts_with(expected),
            "{}",
            fixture.result.diagnostic_snapshot()
        );
    }
}
