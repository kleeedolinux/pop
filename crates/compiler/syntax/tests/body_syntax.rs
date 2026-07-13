use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{
    BinaryOperator, CaptureFunctionSyntax, ExpressionSyntaxKind, NodeKind, StatementSyntaxKind,
    StringSegmentSyntaxKind, UnaryOperator, parse_file, parse_function_body,
    parse_function_signature,
};

fn parse_body(text: &str) -> pop_syntax::FunctionBodySyntax {
    let source = SourceFile::new(FileId::from_raw(0), "src/body.pop", text).expect("source");
    let syntax = parse_file(&source);
    assert!(
        syntax.diagnostics().is_empty(),
        "structural syntax diagnostics: {}",
        syntax.diagnostic_snapshot()
    );
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function declaration");
    let signature = parse_function_signature(&source, &syntax, function).expect("signature");
    parse_function_body(&source, &syntax, function, &signature).expect("body")
}

#[test]
fn parses_typed_and_inferred_locals_and_return() {
    let body = parse_body(
        "namespace Example\n\
         public function add(left: Int, right: Int): Int\n\
             local sum: Int = left + right\n\
             local copy = sum\n\
             return copy\n\
         end\n",
    );

    assert_eq!(body.statements().len(), 3);
    let StatementSyntaxKind::Local {
        name,
        annotation,
        initializer,
    } = body.statements()[0].kind()
    else {
        panic!("expected local statement");
    };
    assert_eq!(name, "sum");
    assert!(annotation.is_some());
    assert!(matches!(
        initializer.kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::Add,
            ..
        }
    ));

    let StatementSyntaxKind::Local {
        name, annotation, ..
    } = body.statements()[1].kind()
    else {
        panic!("expected inferred local");
    };
    assert_eq!(name, "copy");
    assert!(annotation.is_none());

    let StatementSyntaxKind::Return { values } = body.statements()[2].kind() else {
        panic!("expected return statement");
    };
    assert_eq!(values.len(), 1);
    assert!(
        matches!(values[0].kind(), ExpressionSyntaxKind::Name(path) if path.as_slice() == ["copy"])
    );
}

#[test]
fn parses_fixed_pack_returns_multiple_locals_and_multiple_assignment() {
    // ADR 0045: comma-shaped source is exact fixed-pack syntax, not a dynamic
    // variadic carrier or Lua value adjustment.
    let body = parse_body(
        "namespace Example\n\
         public function exchange(left: Int, right: String): (Int, String)\n\
             local first: Int, second: String = left, right\n\
             first, second = left, right\n\
             return first, second\n\
         end\n",
    );

    assert_eq!(body.statements().len(), 3);
    assert!(matches!(
        body.statements()[0].kind(),
        StatementSyntaxKind::MultipleLocal { bindings, values }
            if bindings.len() == 2
                && bindings[0].name() == "first"
                && bindings[0].annotation().is_some()
                && bindings[1].name() == "second"
                && values.len() == 2
    ));
    assert!(matches!(
        body.statements()[1].kind(),
        StatementSyntaxKind::MultipleAssignment { targets, values }
            if targets.len() == 2 && values.len() == 2
    ));
    assert!(matches!(
        body.statements()[2].kind(),
        StatementSyntaxKind::Return { values } if values.len() == 2
    ));
}

#[test]
fn parses_local_and_anonymous_functions_without_table_desugaring() {
    let body = parse_body(
        "namespace Example\n\
         public function make(offset: Int): function(value: Int): Int\n\
             local function add(value: Int): Int\n\
                 return value + offset\n\
             end\n\
             local subtract = function(value: Int): Int\n\
                 return value - offset\n\
             end\n\
             add = subtract\n\
             return add\n\
         end\n",
    );

    let StatementSyntaxKind::LocalFunction { name, function } = body.statements()[0].kind() else {
        panic!("local function");
    };
    assert_eq!(name, "add");
    assert_function_signature(function);

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("anonymous closure local");
    };
    let ExpressionSyntaxKind::Function(function) = initializer.kind() else {
        panic!("anonymous function expression");
    };
    assert_function_signature(function);

    assert!(matches!(
        body.statements()[2].kind(),
        StatementSyntaxKind::Assignment { target, value, .. }
            if matches!(target.kind(), ExpressionSyntaxKind::Name(path) if path == &["add"])
                && matches!(value.kind(), ExpressionSyntaxKind::Name(path) if path == &["subtract"])
    ));
}

fn assert_function_signature(function: &CaptureFunctionSyntax) {
    assert_eq!(function.parameters().len(), 1);
    assert_eq!(function.parameters()[0].name(), "value");
    assert_eq!(function.results().len(), 1);
    assert_eq!(function.body().len(), 1);
}

#[test]
fn parses_exhaustive_luau_shaped_match_arms_and_payload_bindings() {
    let body = parse_body(
        "namespace Example\n\
         public function consume(result: Result<Int, String>)\n\
             match result\n\
             when Result.Ok(value) then\n\
                 use(value)\n\
             when Result.Error(_) then\n\
                 report()\n\
             end\n\
         end\n",
    );

    let StatementSyntaxKind::Match { scrutinee, arms } = body.statements()[0].kind() else {
        panic!("match statement");
    };
    assert!(matches!(scrutinee.kind(), ExpressionSyntaxKind::Name(path) if path == &["result"]));
    assert_eq!(arms.len(), 2);
    assert_eq!(arms[0].case_path(), ["Result", "Ok"]);
    assert_eq!(arms[0].bindings(), ["value"]);
    assert_eq!(arms[0].body().len(), 1);
    assert_eq!(arms[1].case_path(), ["Result", "Error"]);
    assert_eq!(arms[1].bindings(), ["_"]);
}

#[test]
fn calls_respect_arithmetic_precedence() {
    let body = parse_body(
        "namespace Example\n\
         public function calculate(left: Int, right: Int): Int\n\
             return combine(left + 1, right * 2)\n\
         end\n",
    );
    let StatementSyntaxKind::Return { values } = body.statements()[0].kind() else {
        panic!("expected return");
    };
    let ExpressionSyntaxKind::Call { callee, arguments } = values[0].kind() else {
        panic!("expected call");
    };
    assert!(
        matches!(callee.kind(), ExpressionSyntaxKind::Name(path) if path.as_slice() == ["combine"])
    );
    assert_eq!(arguments.len(), 2);
    assert!(matches!(
        arguments[0].kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::Add,
            ..
        }
    ));
    assert!(matches!(
        arguments[1].kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::Multiply,
            ..
        }
    ));
}

#[test]
fn parses_decoded_strings_concatenation_and_interpolation_segments() {
    // ADR 0041: interpolation retains typed expression segments and `..` has
    // lower precedence than arithmetic inside those segments.
    let body = parse_body(
        "namespace Example\n\
         public function describe(count: Int, name: String): String\n\
             return \"line\\n\" .. `checked {count + 1}: {name}`\n\
         end\n",
    );
    let StatementSyntaxKind::Return { values } = body.statements()[0].kind() else {
        panic!("return");
    };
    let ExpressionSyntaxKind::Binary {
        operator: BinaryOperator::Concat,
        left,
        right,
    } = values[0].kind()
    else {
        panic!("string concatenation");
    };
    assert!(matches!(left.kind(), ExpressionSyntaxKind::String(value) if value == "line\n"));
    let ExpressionSyntaxKind::InterpolatedString(segments) = right.kind() else {
        panic!("interpolated string");
    };
    assert_eq!(segments.len(), 4);
    assert!(
        matches!(segments[0].kind(), StringSegmentSyntaxKind::Text(value) if value == "checked ")
    );
    assert!(
        matches!(segments[1].kind(), StringSegmentSyntaxKind::Expression(expression)
        if matches!(expression.kind(), ExpressionSyntaxKind::Binary { operator: BinaryOperator::Add, .. }))
    );
    assert!(matches!(segments[2].kind(), StringSegmentSyntaxKind::Text(value) if value == ": "));
    assert!(
        matches!(segments[3].kind(), StringSegmentSyntaxKind::Expression(expression)
        if matches!(expression.kind(), ExpressionSyntaxKind::Name(path) if path == &["name"]))
    );
}

#[test]
fn parses_typed_attribute_queries_without_string_names() {
    let body = parse_body(
        "namespace Example\n\
         public function inspect(): Boolean\n\
             local present = hasAttribute<<Serializable>>(User)\n\
             local value = attribute<<Serializable>>(User)\n\
             return present and value ~= nil\n\
         end\n",
    );

    for statement in &body.statements()[..2] {
        let StatementSyntaxKind::Local { initializer, .. } = statement.kind() else {
            panic!("query local");
        };
        let ExpressionSyntaxKind::GenericCall {
            callee,
            type_arguments,
            arguments,
        } = initializer.kind()
        else {
            panic!("typed generic call");
        };
        assert!(matches!(callee.kind(), ExpressionSyntaxKind::Name(path) if path.len() == 1));
        assert_eq!(type_arguments.len(), 1);
        assert_eq!(arguments.len(), 1);
        assert!(
            matches!(arguments[0].kind(), ExpressionSyntaxKind::Name(path) if path == &["User"])
        );
    }
}

#[test]
fn equality_uses_luau_tokens_and_comparison_precedence() {
    let body = parse_body(
        "namespace Example\n\
         public function compare(left: Int, right: Int): Boolean\n\
             return left + 1 == right and left ~= 0\n\
         end\n",
    );
    let StatementSyntaxKind::Return { values } = body.statements()[0].kind() else {
        panic!("return");
    };
    let ExpressionSyntaxKind::Binary {
        operator: BinaryOperator::And,
        left,
        right,
    } = values[0].kind()
    else {
        panic!("logical conjunction");
    };
    assert!(matches!(
        left.kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::Equal,
            left,
            ..
        } if matches!(
            left.kind(),
            ExpressionSyntaxKind::Binary {
                operator: BinaryOperator::Add,
                ..
            }
        )
    ));
    assert!(matches!(
        right.kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::NotEqual,
            ..
        }
    ));
}

#[test]
fn parses_optional_binding_defaulting_and_propagation_without_truthiness() {
    // ADR 0051 gives each optional control form a distinct syntax node so
    // later typing/HIR cannot reinterpret it as Boolean truthiness.
    let body = parse_body(
        "namespace Example\n\
         private function choose(primary: String?, secondary: String?): String?\n\
             local selected = primary ?? secondary ?? \"fallback\"\n\
             local propagated = secondary?\n\
             if local value = primary then\n\
                 return value\n\
             else\n\
                 use(propagated)\n\
             end\n\
             while local value = secondary do\n\
                 use(value)\n\
             end\n\
             return selected\n\
         end\n",
    );

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("defaulting local");
    };
    assert!(matches!(
        initializer.kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::OptionalDefault,
            right,
            ..
        } if matches!(
            right.kind(),
            ExpressionSyntaxKind::Binary {
                operator: BinaryOperator::OptionalDefault,
                ..
            }
        )
    ));

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("propagation local");
    };
    assert!(matches!(
        initializer.kind(),
        ExpressionSyntaxKind::OptionalPropagate { operand }
            if matches!(operand.kind(), ExpressionSyntaxKind::Name(path) if path == &["secondary"])
    ));

    assert!(matches!(
        body.statements()[2].kind(),
        StatementSyntaxKind::OptionalIf {
            name,
            initializer,
            then_body,
            else_body,
        } if name == "value"
            && matches!(initializer.kind(), ExpressionSyntaxKind::Name(path) if path == &["primary"])
            && then_body.len() == 1
            && else_body.len() == 1
    ));
    assert!(matches!(
        body.statements()[3].kind(),
        StatementSyntaxKind::OptionalWhile {
            name,
            initializer,
            body,
        } if name == "value"
            && matches!(initializer.kind(), ExpressionSyntaxKind::Name(path) if path == &["secondary"])
            && body.len() == 1
    ));
}

#[test]
fn malformed_optional_binding_reports_an_owned_recovery_expectation() {
    let text = "namespace Example\n\
                private function invalid(value: String?)\n\
                    if local = value then\n\
                    end\n\
                end\n";
    let source =
        SourceFile::new(FileId::from_raw(0), "src/body.pop", text).expect("source fixture");
    let syntax = parse_file(&source);
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function declaration");
    let signature = parse_function_signature(&source, &syntax, function).expect("signature");
    let error = parse_function_body(&source, &syntax, function, &signature)
        .expect_err("malformed optional binding must not become ordinary truthiness");
    assert_eq!(error.expectation(), "optional binding name");
}

#[test]
fn optional_default_precedence_sits_between_or_and_and() {
    let body = parse_body(
        "namespace Example\n\
         private function choose(flag: Boolean, primary: Boolean?, secondary: Boolean): Boolean\n\
             return flag or primary ?? secondary and true\n\
         end\n",
    );
    let StatementSyntaxKind::Return { values } = body.statements()[0].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::Or,
            right,
            ..
        } if matches!(
            right.kind(),
            ExpressionSyntaxKind::Binary {
                operator: BinaryOperator::OptionalDefault,
                right,
                ..
            } if matches!(
                right.kind(),
                ExpressionSyntaxKind::Binary {
                    operator: BinaryOperator::And,
                    ..
                }
            )
        )
    ));
}

#[test]
fn parses_decimal_literals_complete_ordering_and_numeric_cast_calls() {
    // ADR 0040 keeps casts Luau-light as target-type calls while representing
    // decimal literals and complete ordering explicitly in syntax.
    let body = parse_body(
        "namespace Example\n\
         public function convert(count: Int): Boolean\n\
             local ratio = Float64(count)\n\
             local small: Float32 = 1.25\n\
             return ratio <= 6.02e23 and ratio >= 2e-3\n\
         end\n",
    );

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("numeric cast local");
    };
    assert!(matches!(
        initializer.kind(),
        ExpressionSyntaxKind::Call { callee, arguments }
            if matches!(callee.kind(), ExpressionSyntaxKind::Name(path) if path == &["Float64"])
                && arguments.len() == 1
    ));

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("decimal local");
    };
    assert!(matches!(initializer.kind(), ExpressionSyntaxKind::Float(value) if value == "1.25"));

    let StatementSyntaxKind::Return { values } = body.statements()[2].kind() else {
        panic!("comparison return");
    };
    assert!(matches!(
        values[0].kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::And,
            left,
            right,
        } if matches!(left.kind(), ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::LessThanOrEqual,
            right,
            ..
        } if matches!(right.kind(), ExpressionSyntaxKind::Float(value) if value == "6.02e23"))
            && matches!(right.kind(), ExpressionSyntaxKind::Binary {
                operator: BinaryOperator::GreaterThanOrEqual,
                right,
                ..
            } if matches!(right.kind(), ExpressionSyntaxKind::Float(value) if value == "2e-3"))
    ));
}

#[test]
fn parses_logical_unary_literal_and_tuple_expressions() {
    let body = parse_body(
        "namespace Example\n\
         public function values(): (String, nil, Int)\n\
             local state = not false or true\n\
             return (\"value\", nil, -1)\n\
         end\n",
    );

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("expected local");
    };
    let ExpressionSyntaxKind::Binary {
        operator: BinaryOperator::Or,
        left,
        ..
    } = initializer.kind()
    else {
        panic!("expected logical or");
    };
    assert!(matches!(
        left.kind(),
        ExpressionSyntaxKind::Unary {
            operator: UnaryOperator::Not,
            ..
        }
    ));

    let StatementSyntaxKind::Return { values } = body.statements()[1].kind() else {
        panic!("expected return");
    };
    let ExpressionSyntaxKind::Tuple(elements) = values[0].kind() else {
        panic!("expected tuple");
    };
    assert_eq!(elements.len(), 3);
    assert!(matches!(
        elements[0].kind(),
        ExpressionSyntaxKind::String(_)
    ));
    assert!(matches!(elements[1].kind(), ExpressionSyntaxKind::Nil));
    assert!(matches!(
        elements[2].kind(),
        ExpressionSyntaxKind::Unary {
            operator: UnaryOperator::Negate,
            ..
        }
    ));
}

#[test]
fn rejects_a_local_without_an_initializer() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/body.pop",
        "namespace Example\n\
         public function invalid(): Int\n\
             local value: Int\n\
             return value\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let signature = parse_function_signature(&source, &syntax, function).expect("signature");
    let error = parse_function_body(&source, &syntax, function, &signature)
        .expect_err("local initializer is required");

    assert_eq!(error.expectation(), "`=`");
}

#[test]
fn parses_nested_if_else_and_while_blocks() {
    let body = parse_body(
        "namespace Example\n\
         public function choose(condition: Boolean): Int\n\
             if condition then\n\
                 while condition do\n\
                     condition\n\
                 end\n\
                 return 1\n\
             else\n\
                 return 2\n\
             end\n\
         end\n",
    );

    let StatementSyntaxKind::If {
        condition,
        then_body,
        else_body,
    } = body.statements()[0].kind()
    else {
        panic!("expected if statement");
    };
    assert!(matches!(condition.kind(), ExpressionSyntaxKind::Name(_)));
    assert_eq!(then_body.len(), 2);
    assert_eq!(else_body.len(), 1);
    assert!(matches!(
        then_body[0].kind(),
        StatementSyntaxKind::While { body, .. } if body.len() == 1
    ));
}

#[test]
fn parses_lazy_if_expressions_and_elseif_chains() {
    // ADR 0043 uses Luau keywords for both value and statement conditionals.
    let body = parse_body(
        "namespace Example\n\
         public function choose(first: Boolean, second: Boolean): Int\n\
             local value = if first then 1 else if second then 2 else 3\n\
             if first then\n\
                 return value\n\
             elseif second then\n\
                 return 2\n\
             else\n\
                 return 3\n\
             end\n\
         end\n",
    );

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("conditional local");
    };
    assert!(matches!(
        initializer.kind(),
        ExpressionSyntaxKind::Conditional { when_false, .. }
            if matches!(when_false.kind(), ExpressionSyntaxKind::Conditional { .. })
    ));
    let StatementSyntaxKind::If { else_body, .. } = body.statements()[1].kind() else {
        panic!("if statement");
    };
    assert!(matches!(
        else_body.as_slice(),
        [statement] if matches!(statement.kind(), StatementSyntaxKind::If { .. })
    ));
}

#[test]
fn conditional_expressions_require_else_and_reject_ternary_punctuation() {
    for (expression, reason) in [
        (
            "if condition then 1",
            "conditional expression requires else",
        ),
        ("condition ? 1 : 2", "ternary punctuation is not Pop syntax"),
    ] {
        let text = format!(
            "namespace Example\n\
             public function invalid(condition: Boolean): Int\n\
                 return {expression}\n\
             end\n"
        );
        let source =
            SourceFile::new(FileId::from_raw(0), "src/body.pop", text).expect("source fixture");
        let syntax = parse_file(&source);
        let function = syntax
            .root()
            .children()
            .iter()
            .find(|node| node.kind() == NodeKind::FunctionDeclaration)
            .expect("function declaration");
        let signature = parse_function_signature(&source, &syntax, function).expect("signature");
        parse_function_body(&source, &syntax, function, &signature).expect_err(reason);
    }
}

#[test]
fn parses_only_compound_assignments_with_owned_underlying_operators() {
    // ADR 0044 derives compound spellings only from existing Pop operators.
    let body = parse_body(
        "namespace Example\n\
         public function update(value: Int, text: String): Int\n\
             local result = value\n\
             result += 1\n\
             result -= 2\n\
             result *= 3\n\
             result /= 4\n\
             result %= 5\n\
             text ..= \"!\"\n\
             return result\n\
         end\n",
    );

    let expected = [
        BinaryOperator::Add,
        BinaryOperator::Subtract,
        BinaryOperator::Multiply,
        BinaryOperator::Divide,
        BinaryOperator::Remainder,
        BinaryOperator::Concat,
    ];
    for (statement, expected) in body.statements()[1..7].iter().zip(expected) {
        assert!(matches!(
            statement.kind(),
            StatementSyntaxKind::Assignment {
                operator: Some(operator),
                ..
            } if *operator == expected
        ));
    }

    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/body.pop",
        "namespace Example\n\
         public function invalid(value: Int): Int\n\
             value ^= 1\n\
             return value\n\
         end\n",
    )
    .expect("source");
    assert!(
        !parse_file(&source).diagnostics().is_empty(),
        "unsupported underlying operator must remain rejected"
    );
}

#[test]
fn accepts_nested_luau_shaped_repeat_until_without_extra_end_markers() {
    // ADR 0032: `until`, rather than `end`, closes a body-first loop.
    let body = parse_body(
        "namespace Example\n\
         public function count(): Int\n\
             local value = 0\n\
             repeat\n\
                 value = value + 1\n\
                 if value < 2 then\n\
                     repeat\n\
                         value = value + 1\n\
                     until value == 2\n\
                 end\n\
             until value == 3\n\
             return value\n\
         end\n",
    );

    assert_eq!(body.statements().len(), 3);
}

#[test]
fn parses_numeric_for_ranges_and_loop_control_without_dynamic_iteration() {
    // ADR 0042 keeps the first `for` form closed over one fixed integer kind.
    let body = parse_body(
        "namespace Example\n\
         public function visitRange(limit: Int)\n\
             for index = 1, limit do\n\
                 if index == 2 then\n\
                     continue\n\
                 end\n\
                 break\n\
             end\n\
             for reverse = limit, 1, -1 do\n\
                 visit(reverse)\n\
             end\n\
         end\n",
    );

    let StatementSyntaxKind::NumericFor {
        name,
        first,
        last,
        step,
        body: range_body,
    } = body.statements()[0].kind()
    else {
        panic!("numeric for");
    };
    assert_eq!(name, "index");
    assert!(matches!(first.kind(), ExpressionSyntaxKind::Integer(value) if value == "1"));
    assert!(matches!(last.kind(), ExpressionSyntaxKind::Name(path) if path == &["limit"]));
    assert!(step.is_none());
    let StatementSyntaxKind::If { then_body, .. } = range_body[0].kind() else {
        panic!("conditional continue");
    };
    assert!(matches!(then_body[0].kind(), StatementSyntaxKind::Continue));
    assert!(matches!(range_body[1].kind(), StatementSyntaxKind::Break));

    let StatementSyntaxKind::NumericFor { step, .. } = body.statements()[1].kind() else {
        panic!("stepped numeric for");
    };
    assert!(matches!(
        step.as_ref().map(|expression| expression.kind()),
        Some(ExpressionSyntaxKind::Unary {
            operator: UnaryOperator::Negate,
            ..
        })
    ));
}

#[test]
fn repeat_until_requires_its_until_closer() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/body.pop",
        "namespace Example\n\
         public function invalid(): Int\n\
             repeat\n\
                 local value = 1\n\
             end\n\
             return 0\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let signature = parse_function_signature(&source, &syntax, function).expect("signature");
    let error = parse_function_body(&source, &syntax, function, &signature)
        .expect_err("repeat requires `until`, not `end`");

    assert_eq!(error.expectation(), "`until`");
}

#[test]
fn rejects_if_without_then() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/body.pop",
        "namespace Example\n\
         public function invalid(condition: Boolean): Int\n\
             if condition\n\
                 return 1\n\
             end\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let signature = parse_function_signature(&source, &syntax, function).expect("signature");
    let error =
        parse_function_body(&source, &syntax, function, &signature).expect_err("then is required");

    assert_eq!(error.expectation(), "`then`");
}

#[test]
fn parses_lua_shaped_aggregate_literals_and_typed_with_updates() {
    let body = parse_body(
        "namespace Example\n\
         public function update(player: Player): Player\n\
             local replacement: Player = {\n\
                 name = \"Ana\",\n\
                 score = 10,\n\
             }\n\
             return replacement with {\n\
                 score = player.score + 1,\n\
             }\n\
         end\n",
    );

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("aggregate local");
    };
    let ExpressionSyntaxKind::Aggregate { fields } = initializer.kind() else {
        panic!("aggregate literal");
    };
    assert_eq!(
        fields
            .iter()
            .map(pop_syntax::FieldInitializerSyntax::name)
            .collect::<Vec<_>>(),
        ["name", "score"]
    );

    let StatementSyntaxKind::Return { values } = body.statements()[1].kind() else {
        panic!("return");
    };
    let ExpressionSyntaxKind::With { base, fields } = values[0].kind() else {
        panic!("with update");
    };
    assert!(matches!(base.kind(), ExpressionSyntaxKind::Name(_)));
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name(), "score");
}

#[test]
fn named_aggregate_fields_require_values() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/body.pop",
        "namespace Example\n\
         public function invalid(): Player\n\
             return {\n\
                 score =,\n\
             }\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let signature = parse_function_signature(&source, &syntax, function).expect("signature");
    let error = parse_function_body(&source, &syntax, function, &signature)
        .expect_err("field value is required");

    assert_eq!(error.expectation(), "expression");
}

#[test]
fn parses_luau_shaped_array_literals_separately_from_named_aggregates() {
    let body = parse_body(
        "namespace Example\n\
         public function collections(): ({String}, {[String]: Int})\n\
             local names: {String} = { \"Alice\", \"Bruno\" }\n\
             local scores: {[String]: Int} = {\n\
                 alice = 10,\n\
                 bruno = 12,\n\
             }\n\
             return (names, scores)\n\
         end\n",
    );

    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("array local");
    };
    assert!(matches!(
        initializer.kind(),
        ExpressionSyntaxKind::Array(elements) if elements.len() == 2
    ));
    let StatementSyntaxKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("table local");
    };
    assert!(matches!(
        initializer.kind(),
        ExpressionSyntaxKind::Aggregate { fields } if fields.len() == 2
    ));
}

#[test]
fn parses_one_based_collection_indexing_as_a_postfix_expression() {
    let body = parse_body(
        "namespace Example\n\
         public function first(values: {String}): String?\n\
             return values[1]\n\
         end\n",
    );

    let StatementSyntaxKind::Return { values } = body.statements()[0].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        ExpressionSyntaxKind::Index { base, index }
            if matches!(base.kind(), ExpressionSyntaxKind::Name(path) if path == &["values"])
                && matches!(index.kind(), ExpressionSyntaxKind::Integer(value) if value == "1")
    ));
}

#[test]
fn collection_indexing_requires_a_closing_bracket() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/body.pop",
        "namespace Example\n\
         public function invalid(values: {String}): String?\n\
             return values[1\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let signature = parse_function_signature(&source, &syntax, function).expect("signature");
    let error = parse_function_body(&source, &syntax, function, &signature)
        .expect_err("closing bracket is required");

    assert_eq!(error.expectation(), "`]`");
}

#[test]
fn parses_native_class_construction_without_table_call_desugaring() {
    let body = parse_body(
        "namespace Example\n\
         public function make(value: Int): Counter\n\
             return Counter { value = value }\n\
         end\n",
    );

    let StatementSyntaxKind::Return { values } = body.statements()[0].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        ExpressionSyntaxKind::Construct { type_name, fields }
            if type_name == &["Counter"] && fields.len() == 1 && fields[0].name() == "value"
    ));
}

#[test]
fn parses_luau_colon_receiver_calls_separately_from_static_calls() {
    let body = parse_body(
        "namespace Example\n\
         public function read(counter: Counter): Int\n\
             Counter.new(1)\n\
             return counter:get()\n\
         end\n",
    );

    let StatementSyntaxKind::Expression(static_call) = body.statements()[0].kind() else {
        panic!("static call");
    };
    assert!(matches!(
        static_call.kind(),
        ExpressionSyntaxKind::Call { callee, .. }
            if matches!(callee.kind(), ExpressionSyntaxKind::Name(path) if path == &["Counter", "new"])
    ));
    let StatementSyntaxKind::Return { values } = body.statements()[1].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        ExpressionSyntaxKind::MethodCall { receiver, method, arguments }
            if method == "get"
                && arguments.is_empty()
                && matches!(receiver.kind(), ExpressionSyntaxKind::Name(path) if path == &["counter"])
    ));
}

#[test]
fn parses_field_assignment_as_a_statement_not_an_equality_expression() {
    let body = parse_body(
        "namespace Example\n\
         public function set(counter: Counter, value: Int)\n\
             counter.value = value\n\
         end\n",
    );

    assert!(matches!(
        body.statements()[0].kind(),
        StatementSyntaxKind::Assignment { target, value, .. }
            if matches!(target.kind(), ExpressionSyntaxKind::Name(path) if path == &["counter", "value"])
                && matches!(value.kind(), ExpressionSyntaxKind::Name(path) if path == &["value"])
    ));
}
