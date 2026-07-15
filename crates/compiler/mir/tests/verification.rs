use pop_mir::{MirVerificationError, parse_mir_dump, verify_mir_bubble};
use pop_types::TypeArena;

fn diamond_mir(types: &TypeArena) -> String {
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{integer}) -> (t{integer})\n",
            "  b0(v0:t{integer}):\n",
            "    v1:t{boolean} = const.boolean true\n",
            "    condBranch v1 b1 b2\n",
            "  b1():\n",
            "    v2:t{integer} = const.integer Int64 1\n",
            "    branch b3 (v2)\n",
            "  b2():\n",
            "    v3:t{integer} = const.integer Int64 2\n",
            "    branch b3 (v3)\n",
            "  b3(v4:t{integer}):\n",
            "    return (v4)\n",
        ),
        integer = integer.raw(),
        boolean = boolean.raw(),
    )
}

fn foreign_call_mir(types: &TypeArena) -> String {
    let int32 = types.source_type("Int32").expect("Int32");
    format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "foreign s0 f0 params() results(t{int32}) symbol(native_poll) abi(C) links(-) effects[ForeignFunction,UnsafeMemory,GcSafePoint,Blocks]\n",
            "function s1 f1() -> (t{int32}) effects[ForeignFunction,UnsafeMemory,GcSafePoint,Blocks]\n",
            "  b0():\n",
            "    do v0 gcSafePoint sp0 roots ()\n",
            "    v1:t{int32} = callForeign s0 () safePoint sp0 roots () effects[ForeignFunction,UnsafeMemory,GcSafePoint,Blocks] unwind propagate\n",
            "    return (v1)\n",
        ),
        int32 = int32.raw(),
    )
}

#[test]
fn verifier_accepts_typed_diamond_edges_and_join_arguments() {
    let types = TypeArena::new();
    let mir = parse_mir_dump(&diamond_mir(&types)).expect("diamond MIR");

    assert!(verify_mir_bubble(&mir, &types).is_ok());
}

#[test]
fn verifier_rejects_forged_foreign_call_identity_and_root_publication() {
    let types = TypeArena::new();
    let valid = foreign_call_mir(&types);
    let direct = parse_mir_dump(&valid.replace(
        "callForeign s0 () safePoint sp0 roots ()",
        "callDirect s0 ()",
    ))
    .expect("direct call to foreign identity parses");
    let wrong_identity = parse_mir_dump(&valid.replace("callForeign s0", "callForeign s1"))
        .expect("foreign call to Pop identity parses");
    let wrong_roots = parse_mir_dump(&valid.replace(
        "callForeign s0 () safePoint sp0 roots () effects",
        "callForeign s0 () safePoint sp1 roots () effects",
    ))
    .expect("mismatched foreign publication parses");

    assert!(matches!(
        verify_mir_bubble(&direct, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidForeignCall { function, .. } if function.raw() == 0
        ))
    ));
    assert!(matches!(
        verify_mir_bubble(&wrong_identity, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidForeignCall { function, .. } if function.raw() == 1
        ))
    ));
    assert!(matches!(
        verify_mir_bubble(&wrong_roots, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidForeignRoots { .. }
        ))
    ));
}

#[test]
fn verifier_rejects_edge_arity_types_and_non_boolean_conditions() {
    let types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let valid = diamond_mir(&types);
    let cases = [
        (valid.replacen("branch b3 (v2)", "branch b3 ()", 1), "arity"),
        (
            valid.replacen("branch b3 (v2)", "branch b3 (v1)", 1),
            "type",
        ),
        (
            valid.replacen("condBranch v1 b1 b2", "condBranch v0 b1 b2", 1),
            "condition",
        ),
    ];

    for (text, expected) in cases {
        let mir = parse_mir_dump(&text).expect("structurally valid malformed MIR");
        let errors = verify_mir_bubble(&mir, &types).expect_err("semantic MIR error");
        assert!(
            errors.iter().any(|error| match (expected, error) {
                ("arity", MirVerificationError::EdgeArgumentArity { .. }) => true,
                (
                    "type",
                    MirVerificationError::EdgeArgumentType {
                        expected, found, ..
                    },
                ) => *expected == integer && *found == boolean,
                (
                    "condition",
                    MirVerificationError::ConditionalBranchConditionType { found, .. },
                ) => *found == integer,
                _ => false,
            }),
            "{errors:?}"
        );
    }
}

#[test]
fn verifier_rejects_entry_parameter_arity_and_types() {
    let types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let valid = diamond_mir(&types);
    let missing = parse_mir_dump(&valid.replacen(&format!("b0(v0:t{})", integer.raw()), "b0()", 1))
        .expect("missing entry parameter parses");
    let mistyped = parse_mir_dump(&valid.replacen(
        &format!("b0(v0:t{})", integer.raw()),
        &format!("b0(v0:t{})", boolean.raw()),
        1,
    ))
    .expect("mistyped entry parameter parses");

    assert!(matches!(
        verify_mir_bubble(&missing, &types),
        Err(errors) if errors.contains(&MirVerificationError::EntryParameterArity {
            expected: 1,
            found: 0,
        })
    ));
    assert!(matches!(
        verify_mir_bubble(&mistyped, &types),
        Err(errors) if errors.contains(&MirVerificationError::EntryParameterType {
            index: 0,
            expected: integer,
            found: boolean,
        })
    ));
}

#[test]
fn verifier_rejects_a_value_from_a_sibling_control_flow_branch() {
    let types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let text = format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{integer}) -> (t{integer})\n",
            "  b0(v0:t{integer}):\n",
            "    v1:t{boolean} = const.boolean true\n",
            "    condBranch v1 b1 b2\n",
            "  b1():\n",
            "    v2:t{integer} = const.integer Int64 1\n",
            "    return (v2)\n",
            "  b2():\n",
            "    return (v2)\n",
        ),
        integer = integer.raw(),
        boolean = boolean.raw(),
    );
    let mir = parse_mir_dump(&text).expect("structurally valid non-dominating MIR");

    assert!(matches!(
        verify_mir_bubble(&mir, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::ValueNotDominated { value, .. } if value.raw() == 2
        ))
    ));
}

#[test]
fn numeric_text_and_verifier_reject_non_portable_constant_and_operation_types() {
    let types = TypeArena::new();
    let int8 = types.source_type("Int8").expect("Int8");
    let uint8 = types.source_type("UInt8").expect("UInt8");
    let float32 = types.source_type("Float32").expect("Float32");

    let out_of_range = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{})\n  b0():\n    v0:t{} = const.integer UInt8 300\n    return (v0)\n",
        uint8.raw(),
        uint8.raw(),
    );
    assert!(parse_mir_dump(&out_of_range).is_err());

    let wrong_integer_result = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{})\n  b0():\n    v0:t{} = const.integer UInt8 1\n    return (v0)\n",
        int8.raw(),
        int8.raw(),
    );
    let wrong_integer_result = parse_mir_dump(&wrong_integer_result).expect("parseable MIR");
    assert!(matches!(
        verify_mir_bubble(&wrong_integer_result, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::InvalidInstructionType { result_type, .. }
                if *result_type == int8
        ))
    ));

    let float_over_integers = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{int8}, t{int8}) -> (t{int8})\n  b0(v0:t{int8}, v1:t{int8}):\n    v2:t{int8} = float.add Float32 v0 v1\n    return (v2)\n",
        int8 = int8.raw(),
    );
    let float_over_integers = parse_mir_dump(&float_over_integers).expect("parseable MIR");
    assert!(matches!(
        verify_mir_bubble(&float_over_integers, &types),
        Err(errors) if errors.iter().any(|error| matches!(
            error,
            MirVerificationError::WrongOperandType { expected, found, .. }
                if *expected == float32 && *found == int8
        ))
    ));
}
