use std::collections::BTreeSet;

use super::*;
use pop_foundation::{
    BindingId, BubbleId, BuiltinTypeId, CaptureId, ClassId, FieldId, FileId, FunctionId,
    InterfaceId, InterfaceMethodId, IterationCaseId, IterationProtocolMethodId, LocalId, MethodId,
    ModuleId, NamespaceId, NestedFunctionId, ParameterId, SourceSpan, SymbolId, TextRange,
    TextSize, TypeId, UnionCaseId, ValueParameterId,
};
use pop_resolve::Visibility;
use pop_types::{
    ClassMethodDispatch, IntegerKind, IntegerValue, SemanticType, TypeArena, TypedBinaryOperator,
    TypedUnaryOperator,
};

#[test]
fn verifier_rejects_collection_elements_with_inconsistent_types() {
    let mut arena = TypeArena::new();
    let string = arena.source_type("String").expect("String");
    let integer = arena.source_type("Int").expect("Int");
    let array = arena
        .intern(SemanticType::Array(string))
        .expect("array type");
    let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));
    let function = HirFunction {
        function: FunctionId::from_raw(0),
        symbol: SymbolId::from_raw(0),
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "invalid".to_owned(),
        is_async: false,
        type_parameters: Vec::new(),
        type_parameter_names: Vec::new(),
        type_parameter_bounds: Vec::new(),
        parameters: Vec::new(),
        results: Vec::new(),
        body: vec![HirStatement {
            kind: HirStatementKind::Expression(HirExpression {
                kind: HirExpressionKind::Array(vec![HirExpression {
                    kind: HirExpressionKind::Integer(
                        IntegerValue::parse_decimal("1", IntegerKind::Int64).expect("integer"),
                    ),
                    type_id: integer,
                    span,
                }]),
                type_id: array,
                span,
            }),
            span,
        }],
        attributes: Vec::new(),
        effects: pop_types::EffectSummary::empty(),
    };

    assert_eq!(
        verify_hir_function(&function, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::ExpressionTypeMismatch {
            expected: string,
            found: integer,
            span,
        }])
    );
}

#[test]
fn verifier_rejects_array_access_on_a_non_array_base() {
    let mut arena = TypeArena::new();
    let string = arena.source_type("String").expect("String");
    let integer = arena.source_type("Int").expect("Int");
    let optional_string = arena.optional(string).expect("optional string");
    let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));
    let function = HirFunction {
        function: FunctionId::from_raw(0),
        symbol: SymbolId::from_raw(0),
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "invalid".to_owned(),
        is_async: false,
        type_parameters: Vec::new(),
        type_parameter_names: Vec::new(),
        type_parameter_bounds: Vec::new(),
        parameters: Vec::new(),
        results: Vec::new(),
        body: vec![HirStatement {
            kind: HirStatementKind::Expression(HirExpression {
                kind: HirExpressionKind::ArrayGet {
                    array: Box::new(HirExpression {
                        kind: HirExpressionKind::String("value".to_owned()),
                        type_id: string,
                        span,
                    }),
                    index: Box::new(HirExpression {
                        kind: HirExpressionKind::Integer(
                            IntegerValue::parse_decimal("1", IntegerKind::Int64).expect("integer"),
                        ),
                        type_id: integer,
                        span,
                    }),
                },
                type_id: optional_string,
                span,
            }),
            span,
        }],
        attributes: Vec::new(),
        effects: pop_types::EffectSummary::empty(),
    };

    assert_eq!(
        verify_hir_function(&function, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::InvalidCollectionType {
            type_id: string,
            span,
        }])
    );
}

#[test]
fn verifier_rejects_numeric_operator_type_disagreement() {
    let arena = TypeArena::new();
    let int8 = arena.source_type("Int8").expect("Int8");
    let uint8 = arena.source_type("UInt8").expect("UInt8");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

    let mixed = HirExpression {
        kind: HirExpressionKind::Binary {
            operator: TypedBinaryOperator::Add,
            left: Box::new(integer_expression("1", IntegerKind::Int8, int8, span)),
            right: Box::new(integer_expression("1", IntegerKind::UInt8, uint8, span)),
        },
        type_id: int8,
        span,
    };
    assert_eq!(
        verify_expression_statement(mixed, &arena),
        Err(vec![HirVerificationError::InvalidBinaryOperator {
            operator: TypedBinaryOperator::Add,
            left: int8,
            right: uint8,
            result: int8,
            span,
        }])
    );

    let wrong_comparison_result = HirExpression {
        kind: HirExpressionKind::Binary {
            operator: TypedBinaryOperator::LessThan,
            left: Box::new(integer_expression("1", IntegerKind::Int8, int8, span)),
            right: Box::new(integer_expression("2", IntegerKind::Int8, int8, span)),
        },
        type_id: int8,
        span,
    };
    assert_eq!(
        verify_expression_statement(wrong_comparison_result, &arena),
        Err(vec![HirVerificationError::InvalidBinaryOperator {
            operator: TypedBinaryOperator::LessThan,
            left: int8,
            right: int8,
            result: int8,
            span,
        }])
    );

    let unsigned_negation = HirExpression {
        kind: HirExpressionKind::Unary {
            operator: TypedUnaryOperator::Negate,
            operand: Box::new(integer_expression("1", IntegerKind::UInt8, uint8, span)),
        },
        type_id: uint8,
        span,
    };
    assert_eq!(
        verify_expression_statement(unsigned_negation, &arena),
        Err(vec![HirVerificationError::InvalidUnaryOperator {
            operator: TypedUnaryOperator::Negate,
            operand: uint8,
            result: uint8,
            span,
        }])
    );

    let numeric_boolean = HirExpression {
        kind: HirExpressionKind::Binary {
            operator: TypedBinaryOperator::And,
            left: Box::new(integer_expression("1", IntegerKind::Int8, int8, span)),
            right: Box::new(integer_expression("2", IntegerKind::Int8, int8, span)),
        },
        type_id: boolean,
        span,
    };
    assert_eq!(
        verify_expression_statement(numeric_boolean, &arena),
        Err(vec![HirVerificationError::InvalidBinaryOperator {
            operator: TypedBinaryOperator::And,
            left: int8,
            right: int8,
            result: boolean,
            span,
        }])
    );
}

#[test]
fn verifier_rejects_local_and_return_type_disagreement() {
    let arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

    let local_mismatch = hir_function(
        vec![],
        vec![],
        vec![HirStatement {
            kind: HirStatementKind::Local {
                binding: BindingId::from_raw(0),
                local: LocalId::from_raw(0),
                name: "value".to_owned(),
                local_type: integer,
                initializer: string_expression(string, span),
            },
            span,
        }],
    );
    assert_eq!(
        verify_hir_function(&local_mismatch, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::ExpressionTypeMismatch {
            expected: integer,
            found: string,
            span,
        }])
    );

    let wrong_return = hir_function(
        vec![],
        vec![integer],
        vec![HirStatement {
            kind: HirStatementKind::Return {
                values: vec![string_expression(string, span)],
            },
            span,
        }],
    );
    assert_eq!(
        verify_hir_function(&wrong_return, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::ExpressionTypeMismatch {
            expected: integer,
            found: string,
            span,
        }])
    );

    let missing_return_value = hir_function(
        vec![],
        vec![integer],
        vec![HirStatement {
            kind: HirStatementKind::Return { values: Vec::new() },
            span,
        }],
    );
    assert_eq!(
        verify_hir_function(&missing_return_value, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::WrongReturnArity {
            expected: 1,
            found: 0,
            span,
        }])
    );
}

#[test]
fn verifier_rejects_condition_and_parameter_type_disagreement() {
    let arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

    let numeric_condition = hir_function(
        vec![],
        vec![],
        vec![HirStatement {
            kind: HirStatementKind::If {
                condition: integer_expression("1", IntegerKind::Int64, integer, span),
                then_body: Vec::new(),
                else_body: Vec::new(),
            },
            span,
        }],
    );
    assert_eq!(
        verify_hir_function(&numeric_condition, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::InvalidConditionType {
            found: integer,
            span,
        }])
    );

    let wrong_parameter_type = hir_function(
        vec![HirParameter {
            binding: BindingId::from_raw(0),
            parameter: ValueParameterId::from_raw(0),
            name: "value".to_owned(),
            type_id: integer,
            span,
        }],
        vec![],
        vec![HirStatement {
            kind: HirStatementKind::Expression(HirExpression {
                kind: HirExpressionKind::Parameter(ValueParameterId::from_raw(0)),
                type_id: string,
                span,
            }),
            span,
        }],
    );
    assert_eq!(
        verify_hir_function(&wrong_parameter_type, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::ExpressionTypeMismatch {
            expected: integer,
            found: string,
            span,
        }])
    );
}

#[test]
fn verifier_rejects_literal_and_tuple_type_disagreement() {
    let mut arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let tuple = arena
        .intern(SemanticType::Tuple(vec![integer, string]))
        .expect("tuple");
    let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

    let wrong_tuple_element = hir_function(
        vec![],
        vec![],
        vec![HirStatement {
            kind: HirStatementKind::Expression(HirExpression {
                kind: HirExpressionKind::Tuple(vec![
                    integer_expression("1", IntegerKind::Int64, integer, span),
                    integer_expression("2", IntegerKind::Int64, integer, span),
                ]),
                type_id: tuple,
                span,
            }),
            span,
        }],
    );
    assert_eq!(
        verify_hir_function(&wrong_tuple_element, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::ExpressionTypeMismatch {
            expected: string,
            found: integer,
            span,
        }])
    );

    let wrong_literal_type = hir_function(
        vec![],
        vec![],
        vec![HirStatement {
            kind: HirStatementKind::Expression(string_expression(integer, span)),
            span,
        }],
    );
    assert_eq!(
        verify_hir_function(&wrong_literal_type, &arena, &BTreeSet::new()),
        Err(vec![HirVerificationError::InvalidType {
            type_id: integer,
            span,
        }])
    );
}

#[test]
fn bubble_verifier_rejects_direct_call_argument_and_result_spoofing() {
    let arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let span = test_span();
    let target_function = hir_function_with_symbol(
        SymbolId::from_raw(1),
        vec![hir_parameter(0, "value", integer, span)],
        vec![integer],
        vec![HirStatement {
            kind: HirStatementKind::Return {
                values: vec![parameter_expression(0, integer, span)],
            },
            span,
        }],
    );
    let invoking_function = hir_function_with_symbol(
        SymbolId::from_raw(2),
        Vec::new(),
        vec![string],
        vec![
            HirStatement {
                kind: HirStatementKind::Call(HirCall {
                    is_async: false,
                    dispatch: HirCallDispatch::Direct {
                        function: SymbolId::from_raw(1),
                    },
                    type_arguments: Vec::new(),
                    arguments: Vec::new(),
                    span,
                }),
                span,
            },
            HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![HirExpression {
                        kind: HirExpressionKind::Call {
                            is_async: false,
                            dispatch: HirCallDispatch::Direct {
                                function: SymbolId::from_raw(1),
                            },
                            type_arguments: Vec::new(),
                            arguments: vec![string_expression(string, span)],
                        },
                        type_id: string,
                        span,
                    }],
                },
                span,
            },
        ],
    );
    let bubble = test_bubble(
        Vec::new(),
        vec![target_function, invoking_function],
        Vec::new(),
    );

    assert!(matches!(
        verify_hir_bubble(&bubble, &arena),
        Err(errors)
            if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InvalidCallSignature {
                    expected_arguments: 1,
                    found_arguments: 0,
                    expected_results: 1,
                    found_results: 0,
                    ..
                }
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallArgumentTypeMismatch {
                    index: 0,
                    expected,
                    found,
                    ..
                } if *expected == integer && *found == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallResultTypeMismatch {
                    expected,
                    found,
                    ..
                } if *expected == integer && *found == string
            ))
    ));
}

#[test]
fn bubble_verifier_rejects_indirect_call_argument_and_result_spoofing() {
    let mut arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let callable = arena
        .intern(SemanticType::Function {
            is_async: false,
            parameters: vec![integer],
            results: vec![integer],
            effects: pop_types::EffectSummary::empty(),
        })
        .expect("function type");
    let span = test_span();
    let caller = hir_function_with_symbol(
        SymbolId::from_raw(0),
        vec![hir_parameter(0, "operation", callable, span)],
        vec![string],
        vec![
            HirStatement {
                kind: HirStatementKind::Call(HirCall {
                    is_async: false,
                    dispatch: HirCallDispatch::Indirect {
                        callee: Box::new(parameter_expression(0, callable, span)),
                    },
                    type_arguments: Vec::new(),
                    arguments: Vec::new(),
                    span,
                }),
                span,
            },
            HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![HirExpression {
                        kind: HirExpressionKind::Call {
                            is_async: false,
                            dispatch: HirCallDispatch::Indirect {
                                callee: Box::new(parameter_expression(0, callable, span)),
                            },
                            type_arguments: Vec::new(),
                            arguments: vec![string_expression(string, span)],
                        },
                        type_id: string,
                        span,
                    }],
                },
                span,
            },
        ],
    );
    let bubble = test_bubble(Vec::new(), vec![caller], Vec::new());

    assert!(matches!(
        verify_hir_bubble(&bubble, &arena),
        Err(errors)
            if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InvalidCallSignature {
                    expected_arguments: 1,
                    found_arguments: 0,
                    expected_results: 1,
                    found_results: 0,
                    ..
                }
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallArgumentTypeMismatch {
                    index: 0,
                    expected,
                    found,
                    ..
                } if *expected == integer && *found == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallResultTypeMismatch {
                    expected,
                    found,
                    ..
                } if *expected == integer && *found == string
            ))
    ));
}

#[test]
fn bubble_verifier_rejects_spoofed_function_reference_type() {
    let arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let span = test_span();
    let callee = hir_function_with_symbol(
        SymbolId::from_raw(1),
        vec![hir_parameter(0, "value", integer, span)],
        vec![integer],
        vec![HirStatement {
            kind: HirStatementKind::Return {
                values: vec![parameter_expression(0, integer, span)],
            },
            span,
        }],
    );
    let observer = hir_function_with_symbol(
        SymbolId::from_raw(2),
        Vec::new(),
        Vec::new(),
        vec![HirStatement {
            kind: HirStatementKind::Expression(HirExpression {
                kind: HirExpressionKind::Function(SymbolId::from_raw(1)),
                type_id: string,
                span,
            }),
            span,
        }],
    );
    let bubble = test_bubble(Vec::new(), vec![callee, observer], Vec::new());

    assert!(matches!(
        verify_hir_bubble(&bubble, &arena),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            HirVerificationError::InvalidFunctionReferenceType {
                function,
                found,
                ..
            } if *function == SymbolId::from_raw(1) && *found == string
        ))
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn bubble_verifier_checks_receiver_method_signatures_against_class_schema() {
    let mut arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let class = ClassId::from_raw(0);
    let class_type = arena
        .intern(SemanticType::Class {
            class,
            arguments: Vec::new(),
        })
        .expect("class type");
    let span = test_span();
    let definition = SymbolId::from_raw(10);
    let method = MethodId::from_raw(0);
    let declaration = HirDeclaration {
        symbol: definition,
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Public,
        name: "Counter".to_owned(),
        kind: HirDeclarationKind::Class(HirClassDeclaration {
            class,
            type_id: class_type,
            is_open: false,
            interfaces: Vec::new(),
            builtin_interfaces: Vec::new(),
            fields: Vec::new(),
            methods: vec![HirClassMethod {
                method,
                visibility: Visibility::Public,
                name: "apply".to_owned(),
                dispatch: ClassMethodDispatch::Receiver,
                parameters: vec![HirNamedType {
                    name: "value".to_owned(),
                    type_id: integer,
                    span,
                }],
                results: vec![integer],
                span,
            }],
        }),
        span,
    };
    let method_body = HirMethod {
        method,
        class,
        definition,
        function: hir_function_with_symbol(
            definition,
            vec![
                hir_parameter(0, "self", class_type, span),
                hir_parameter(1, "value", integer, span),
            ],
            vec![integer],
            vec![HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![parameter_expression(1, integer, span)],
                },
                span,
            }],
        ),
    };
    let caller = hir_function_with_symbol(
        SymbolId::from_raw(20),
        Vec::new(),
        vec![string],
        vec![
            HirStatement {
                kind: HirStatementKind::Call(HirCall {
                    is_async: false,
                    dispatch: HirCallDispatch::DirectMethod { method },
                    type_arguments: Vec::new(),
                    arguments: vec![string_expression(string, span)],
                    span,
                }),
                span,
            },
            HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![HirExpression {
                        kind: HirExpressionKind::Call {
                            is_async: false,
                            dispatch: HirCallDispatch::DirectMethod { method },
                            type_arguments: Vec::new(),
                            arguments: vec![
                                string_expression(string, span),
                                string_expression(string, span),
                            ],
                        },
                        type_id: string,
                        span,
                    }],
                },
                span,
            },
        ],
    );
    let bubble = test_bubble(vec![declaration], vec![caller], vec![method_body]);

    assert!(matches!(
        verify_hir_bubble(&bubble, &arena),
        Err(errors)
            if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InvalidCallSignature {
                    expected_arguments: 2,
                    found_arguments: 1,
                    expected_results: 1,
                    found_results: 0,
                    ..
                }
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallArgumentTypeMismatch {
                    index: 0,
                    expected,
                    found,
                    ..
                } if *expected == class_type && *found == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallArgumentTypeMismatch {
                    index: 1,
                    expected,
                    found,
                    ..
                } if *expected == integer && *found == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallResultTypeMismatch {
                    expected,
                    found,
                    ..
                } if *expected == integer && *found == string
            ))
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn bubble_verifier_checks_declaration_field_and_union_case_schema() {
    let mut arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let record_type = arena
        .intern(SemanticType::Record(vec![("value".to_owned(), integer)]))
        .expect("record type");
    let union_symbol = SymbolId::from_raw(11);
    let union_type = arena
        .intern(SemanticType::TaggedUnion {
            definition: union_symbol,
            source: union_symbol,
            arguments: Vec::new(),
        })
        .expect("union type");
    let span = test_span();
    let field = FieldId::from_raw(0);
    let case = UnionCaseId::from_raw(0);
    let record = HirDeclaration {
        symbol: SymbolId::from_raw(10),
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "Data".to_owned(),
        kind: HirDeclarationKind::Record(HirRecordDeclaration {
            type_id: record_type,
            fields: vec![HirRecordField {
                field,
                name: "value".to_owned(),
                field_type: integer,
                default: None,
                span,
            }],
            ffi_c_layout: false,
        }),
        span,
    };
    let union = HirDeclaration {
        symbol: union_symbol,
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "Choice".to_owned(),
        kind: HirDeclarationKind::Union(HirUnionDeclaration {
            type_id: union_type,
            cases: vec![HirUnionCase {
                case,
                name: "Value".to_owned(),
                parameters: vec![HirNamedType {
                    name: "value".to_owned(),
                    type_id: integer,
                    span,
                }],
                span,
            }],
        }),
        span,
    };
    let invalid_union = HirDeclaration {
        symbol: SymbolId::from_raw(12),
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "InvalidChoice".to_owned(),
        kind: HirDeclarationKind::Union(HirUnionDeclaration {
            type_id: record_type,
            cases: Vec::new(),
        }),
        span,
    };
    let function = hir_function_with_symbol(
        SymbolId::from_raw(20),
        Vec::new(),
        Vec::new(),
        vec![
            HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Record {
                        record: SymbolId::from_raw(10),
                        fields: Vec::new(),
                    },
                    type_id: record_type,
                    span,
                }),
                span,
            },
            HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Field {
                        base: Box::new(string_expression(string, span)),
                        field,
                    },
                    type_id: integer,
                    span,
                }),
                span,
            },
            HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::UnionCase {
                        union: union_symbol,
                        case,
                        arguments: vec![string_expression(string, span)],
                    },
                    type_id: union_type,
                    span,
                }),
                span,
            },
            HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::UnionCase {
                        union: union_symbol,
                        case: UnionCaseId::from_raw(99),
                        arguments: Vec::new(),
                    },
                    type_id: union_type,
                    span,
                }),
                span,
            },
        ],
    );
    let bubble = test_bubble(
        vec![record, union, invalid_union],
        vec![function],
        Vec::new(),
    );

    assert!(matches!(
        verify_hir_bubble(&bubble, &arena),
        Err(errors)
            if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InvalidDeclarationType { symbol, type_id, .. }
                    if *symbol == SymbolId::from_raw(12) && *type_id == record_type
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::MissingDeclaredField { field: missing, .. }
                    if *missing == field
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::WrongFieldOwner { field: wrong, found, .. }
                    if *wrong == field && *found == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::UnionCaseArgumentTypeMismatch {
                    union,
                    case: found_case,
                    index: 0,
                    expected,
                    found,
                    ..
                } if *union == union_symbol
                    && *found_case == case
                    && *expected == integer
                    && *found == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::UnknownUnionCase { union, case, .. }
                    if *union == union_symbol && *case == UnionCaseId::from_raw(99)
            ))
    ));
}

#[test]
fn closure_verifier_rejects_duplicate_mistyped_and_wrongly_owned_captures() {
    let mut arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let closure_type = arena
        .intern(SemanticType::Function {
            is_async: false,
            parameters: Vec::new(),
            results: Vec::new(),
            effects: pop_types::EffectSummary::empty(),
        })
        .expect("closure type");
    let span = test_span();
    let capture = CaptureId::from_raw(0);
    let function = hir_function(
        vec![hir_parameter(0, "value", integer, span)],
        Vec::new(),
        vec![HirStatement {
            kind: HirStatementKind::Expression(HirExpression {
                kind: HirExpressionKind::Closure(HirClosure {
                    function: NestedFunctionId::from_raw(0),
                    is_async: false,
                    parameters: Vec::new(),
                    results: Vec::new(),
                    captures: vec![
                        HirCapture {
                            capture,
                            binding: BindingId::from_raw(0),
                            source: HirCaptureSource::Parameter(ValueParameterId::from_raw(0)),
                            type_id: string,
                            mode: HirCaptureMode::Value,
                        },
                        HirCapture {
                            capture,
                            binding: BindingId::from_raw(0),
                            source: HirCaptureSource::Local(LocalId::from_raw(99)),
                            type_id: integer,
                            mode: HirCaptureMode::Cell,
                        },
                    ],
                    body: vec![HirStatement {
                        kind: HirStatementKind::Expression(HirExpression {
                            kind: HirExpressionKind::Capture(CaptureId::from_raw(99)),
                            type_id: integer,
                            span,
                        }),
                        span,
                    }],
                    span,
                    effects: pop_types::EffectSummary::empty(),
                }),
                type_id: closure_type,
                span,
            }),
            span,
        }],
    );

    assert!(matches!(
        verify_hir_function(&function, &arena, &BTreeSet::new()),
        Err(errors)
            if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CaptureTypeMismatch {
                    capture: found,
                    expected,
                    found: found_type,
                    ..
                } if *found == capture && *expected == integer && *found_type == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::DuplicateCapture(found) if *found == capture
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InvalidCaptureSource { capture: found, .. }
                    if *found == capture
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::UnknownCapture { capture: found, .. }
                    if *found == CaptureId::from_raw(99)
            ))
    ));
}

#[test]
fn match_verifier_rejects_duplicate_missing_and_mistyped_case_tables() {
    let mut arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let union_symbol = SymbolId::from_raw(10);
    let union_type = arena
        .intern(SemanticType::TaggedUnion {
            definition: union_symbol,
            source: union_symbol,
            arguments: Vec::new(),
        })
        .expect("union type");
    let span = test_span();
    let first_case = UnionCaseId::from_raw(0);
    let second_case = UnionCaseId::from_raw(1);
    let union = HirDeclaration {
        symbol: union_symbol,
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "ResultValue".to_owned(),
        kind: HirDeclarationKind::Union(HirUnionDeclaration {
            type_id: union_type,
            cases: vec![
                HirUnionCase {
                    case: first_case,
                    name: "Value".to_owned(),
                    parameters: vec![HirNamedType {
                        name: "value".to_owned(),
                        type_id: integer,
                        span,
                    }],
                    span,
                },
                HirUnionCase {
                    case: second_case,
                    name: "Empty".to_owned(),
                    parameters: Vec::new(),
                    span,
                },
            ],
        }),
        span,
    };
    let invalid_binding = |binding, local| HirMatchBinding {
        binding: Some(BindingId::from_raw(binding)),
        local: Some(LocalId::from_raw(local)),
        name: "payload".to_owned(),
        type_id: string,
        span,
    };
    let function = hir_function_with_symbol(
        SymbolId::from_raw(20),
        vec![hir_parameter(0, "result", union_type, span)],
        Vec::new(),
        vec![HirStatement {
            kind: HirStatementKind::Match {
                scrutinee: parameter_expression(0, union_type, span),
                union: union_symbol,
                arms: vec![
                    HirMatchArm {
                        union: union_symbol,
                        case: first_case,
                        bindings: vec![invalid_binding(1, 0)],
                        body: Vec::new(),
                        span,
                    },
                    HirMatchArm {
                        union: union_symbol,
                        case: first_case,
                        bindings: vec![invalid_binding(2, 1)],
                        body: Vec::new(),
                        span,
                    },
                ],
            },
            span,
        }],
    );
    let bubble = test_bubble(vec![union], vec![function], Vec::new());

    assert!(matches!(
        verify_hir_bubble(&bubble, &arena),
        Err(errors)
            if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::DuplicateMatchCase { case, .. }
                    if *case == first_case
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::MissingMatchCase { case, .. }
                    if *case == second_case
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::MatchPayloadTypeMismatch {
                    case,
                    expected,
                    found,
                    ..
                } if *case == first_case && *expected == integer && *found == string
            ))
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn interface_verifier_rejects_wrong_slots_mappings_arguments_and_results() {
    let mut arena = TypeArena::new();
    let integer = arena.source_type("Int").expect("Int");
    let string = arena.source_type("String").expect("String");
    let interface_id = InterfaceId::from_raw(0);
    let interface_type = arena
        .intern(SemanticType::Interface {
            interface: interface_id,
            arguments: Vec::new(),
        })
        .expect("interface type");
    let iterator_type = arena
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(107),
            arguments: vec![integer],
        })
        .expect("iterator type");
    let class_id = ClassId::from_raw(0);
    let class_type = arena
        .intern(SemanticType::Class {
            class: class_id,
            arguments: Vec::new(),
        })
        .expect("class type");
    let span = test_span();
    let interface_method = InterfaceMethodId::from_raw(7);
    let class_method = MethodId::from_raw(3);
    let interface = HirDeclaration {
        symbol: SymbolId::from_raw(10),
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Public,
        name: "Reader".to_owned(),
        kind: HirDeclarationKind::Interface(HirInterfaceDeclaration {
            interface: interface_id,
            type_id: interface_type,
            methods: vec![HirInterfaceMethod {
                method: interface_method,
                slot: 0,
                name: "read".to_owned(),
                parameters: vec![HirNamedType {
                    name: "count".to_owned(),
                    type_id: integer,
                    span,
                }],
                results: vec![string],
                span,
            }],
        }),
        span,
    };
    let class_symbol = SymbolId::from_raw(11);
    let class = HirDeclaration {
        symbol: class_symbol,
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Public,
        name: "FileReader".to_owned(),
        kind: HirDeclarationKind::Class(HirClassDeclaration {
            class: class_id,
            type_id: class_type,
            is_open: false,
            interfaces: vec![HirInterfaceImplementation {
                interface: interface_id,
                interface_type,
                methods: vec![HirInterfaceMethodImplementation {
                    interface_method,
                    slot: 9,
                    class_method,
                }],
            }],
            builtin_interfaces: vec![HirBuiltinInterfaceImplementation {
                interface: BuiltinTypeId::from_raw(107),
                interface_type: iterator_type,
                methods: vec![HirBuiltinInterfaceMethodImplementation {
                    protocol_method: IterationProtocolMethodId::from_raw(9),
                    class_method,
                }],
            }],
            fields: Vec::new(),
            methods: vec![HirClassMethod {
                method: class_method,
                visibility: Visibility::Public,
                name: "read".to_owned(),
                dispatch: ClassMethodDispatch::Receiver,
                parameters: vec![HirNamedType {
                    name: "count".to_owned(),
                    type_id: string,
                    span,
                }],
                results: vec![integer],
                span,
            }],
        }),
        span,
    };
    let method_body = HirMethod {
        method: class_method,
        class: class_id,
        definition: class_symbol,
        function: hir_function_with_symbol(
            class_symbol,
            vec![
                hir_parameter(0, "self", class_type, span),
                hir_parameter(1, "count", string, span),
            ],
            vec![integer],
            vec![HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![integer_expression("0", IntegerKind::Int64, integer, span)],
                },
                span,
            }],
        ),
    };
    let caller = hir_function_with_symbol(
        SymbolId::from_raw(20),
        vec![hir_parameter(0, "reader", interface_type, span)],
        vec![integer],
        vec![HirStatement {
            kind: HirStatementKind::Return {
                values: vec![HirExpression {
                    kind: HirExpressionKind::Call {
                        is_async: false,
                        dispatch: HirCallDispatch::InterfaceMethod {
                            interface: interface_id,
                            method: interface_method,
                            slot: 8,
                        },
                        type_arguments: Vec::new(),
                        arguments: vec![
                            parameter_expression(0, interface_type, span),
                            string_expression(string, span),
                        ],
                    },
                    type_id: integer,
                    span,
                }],
            },
            span,
        }],
    );
    let bubble = test_bubble(vec![interface, class], vec![caller], vec![method_body]);

    assert!(matches!(
        verify_hir_bubble(&bubble, &arena),
        Err(errors)
            if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::WrongInterfaceMethodSlot {
                    method,
                    expected: 0,
                    found: 8 | 9,
                    ..
                } if *method == interface_method
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InterfaceMethodMappingMismatch {
                    method,
                    class_method: found,
                    ..
                } if *method == interface_method && *found == class_method
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallArgumentTypeMismatch {
                    index: 1,
                    expected,
                    found,
                    ..
                } if *expected == integer && *found == string
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::CallResultTypeMismatch {
                    expected,
                    found,
                    ..
                } if *expected == string && *found == integer
            )) && errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InvalidBuiltinInterfaceImplementation {
                    class,
                    interface,
                } if *class == class_id && *interface == BuiltinTypeId::from_raw(107)
            ))
    ));
}

#[test]
fn verifier_rejects_spoofed_iteration_case_and_method_identities() {
    let mut arena = TypeArena::new();
    let item_type = arena.source_type("Int").expect("Int");
    let array_type = arena
        .intern(SemanticType::Array(item_type))
        .expect("array type");
    let iterator_type = arena
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(107),
            arguments: vec![item_type],
        })
        .expect("iterator type");
    let iteration_type = arena
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(113),
            arguments: vec![item_type],
        })
        .expect("iteration type");
    let span = test_span();
    let statement = |item_case, next_method| HirStatement {
        kind: HirStatementKind::GeneralizedFor {
            protocol: HirIterationProtocol {
                iteration: BuiltinTypeId::from_raw(113),
                iterable: BuiltinTypeId::from_raw(106),
                iterator: BuiltinTypeId::from_raw(107),
                list: BuiltinTypeId::from_raw(101),
                range: BuiltinTypeId::from_raw(103),
                item_case,
                end_case: IterationCaseId::from_raw(1),
                iterator_method: IterationProtocolMethodId::from_raw(0),
                next_method,
            },
            source: HirIterationSource::Array,
            item_type,
            iterator_type,
            iteration_type,
            bindings: vec![HirLocalBinding {
                binding: BindingId::from_raw(1),
                local: LocalId::from_raw(0),
                name: "value".to_owned(),
                local_type: item_type,
                span,
            }],
            iterable: parameter_expression(0, array_type, span),
            body: Vec::new(),
        },
        span,
    };

    for invalid in [
        statement(
            IterationCaseId::from_raw(9),
            IterationProtocolMethodId::from_raw(1),
        ),
        statement(
            IterationCaseId::from_raw(0),
            IterationProtocolMethodId::from_raw(9),
        ),
    ] {
        let function = hir_function(
            vec![hir_parameter(0, "values", array_type, span)],
            Vec::new(),
            vec![invalid],
        );
        assert!(matches!(
            verify_hir_function(&function, &arena, &BTreeSet::new()),
            Err(errors) if errors.contains(&HirVerificationError::InvalidIterationProtocol { span })
        ));
    }
}

fn hir_function(
    parameters: Vec<HirParameter>,
    results: Vec<TypeId>,
    body: Vec<HirStatement>,
) -> HirFunction {
    hir_function_with_symbol(SymbolId::from_raw(0), parameters, results, body)
}

fn hir_function_with_symbol(
    symbol: SymbolId,
    parameters: Vec<HirParameter>,
    results: Vec<TypeId>,
    body: Vec<HirStatement>,
) -> HirFunction {
    HirFunction {
        function: FunctionId::from_raw(symbol.raw()),
        symbol,
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "invalid".to_owned(),
        is_async: false,
        type_parameters: Vec::new(),
        type_parameter_names: Vec::new(),
        type_parameter_bounds: Vec::new(),
        parameters,
        results,
        body,
        attributes: Vec::new(),
        effects: pop_types::EffectSummary::empty(),
    }
}

fn hir_parameter(raw: u32, name: &str, type_id: TypeId, span: SourceSpan) -> HirParameter {
    HirParameter {
        binding: BindingId::from_raw(raw),
        parameter: ValueParameterId::from_raw(raw),
        name: name.to_owned(),
        type_id,
        span,
    }
}

fn parameter_expression(raw: u32, type_id: TypeId, span: SourceSpan) -> HirExpression {
    HirExpression {
        kind: HirExpressionKind::Parameter(ValueParameterId::from_raw(raw)),
        type_id,
        span,
    }
}

fn test_bubble(
    declarations: Vec<HirDeclaration>,
    functions: Vec<HirFunction>,
    methods: Vec<HirMethod>,
) -> HirBubble {
    HirBubble::new_with_declarations_and_methods(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        declarations,
        functions,
        methods,
    )
    .expect("structurally assembled test Bubble")
}

fn test_span() -> SourceSpan {
    SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)))
}

fn string_expression(type_id: TypeId, span: SourceSpan) -> HirExpression {
    HirExpression {
        kind: HirExpressionKind::String("value".to_owned()),
        type_id,
        span,
    }
}

fn integer_expression(
    text: &str,
    kind: IntegerKind,
    type_id: TypeId,
    span: SourceSpan,
) -> HirExpression {
    HirExpression {
        kind: HirExpressionKind::Integer(IntegerValue::parse_decimal(text, kind).expect("integer")),
        type_id,
        span,
    }
}

fn verify_expression_statement(
    expression: HirExpression,
    arena: &TypeArena,
) -> Result<(), Vec<HirVerificationError>> {
    let span = expression.span();
    let function = HirFunction {
        function: FunctionId::from_raw(0),
        symbol: SymbolId::from_raw(0),
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "invalid".to_owned(),
        is_async: false,
        type_parameters: Vec::new(),
        type_parameter_names: Vec::new(),
        type_parameter_bounds: Vec::new(),
        parameters: Vec::new(),
        results: Vec::new(),
        body: vec![HirStatement {
            kind: HirStatementKind::Expression(expression),
            span,
        }],
        attributes: Vec::new(),
        effects: pop_types::EffectSummary::empty(),
    };
    verify_hir_function(&function, arena, &BTreeSet::new())
}

#[test]
fn verifier_rejects_generic_parameter_bound_arity_mismatch() {
    let mut arena = TypeArena::new();
    let parameter = arena
        .intern(SemanticType::TypeParameter(ParameterId::from_raw(0)))
        .expect("type parameter");
    let function = HirFunction {
        function: FunctionId::from_raw(0),
        symbol: SymbolId::from_raw(0),
        module: ModuleId::from_raw(0),
        bubble: BubbleId::from_raw(0),
        visibility: Visibility::Private,
        name: "invalidBounds".to_owned(),
        is_async: false,
        type_parameters: vec![parameter],
        type_parameter_names: vec!["T".to_owned()],
        type_parameter_bounds: Vec::new(),
        parameters: Vec::new(),
        results: Vec::new(),
        body: Vec::new(),
        attributes: Vec::new(),
        effects: pop_types::EffectSummary::empty(),
    };

    let errors =
        verify_hir_function(&function, &arena, &BTreeSet::new()).expect_err("bound arity mismatch");
    assert!(matches!(
        errors.as_slice(),
        [HirVerificationError::InvalidGenericBounds { function: found, .. }]
            if *found == SymbolId::from_raw(0)
    ));
}
