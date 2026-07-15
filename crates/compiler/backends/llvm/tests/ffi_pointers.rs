use pop_backend_llvm::{LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_mir::parse_mir_dump;
use pop_target::TargetSpec;
use pop_types::{
    FFI_OPTIONAL_POINTER_TYPE_ID, FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID, FFI_POINTER_TYPE_ID,
    FFI_READ_ONLY_POINTER_TYPE_ID, SemanticType, TypeArena,
};

fn target() -> TargetSpec {
    TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target")
}

#[test]
fn lowers_safe_pointer_construction_and_presence_to_native_values() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("pointer");
    let optional_pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_OPTIONAL_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("optional pointer");
    let read_only_pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_READ_ONLY_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("read-only pointer");
    let optional_read_only_pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("optional read-only pointer");
    let null_pointer_error = types
        .intern(SemanticType::Builtin {
            definition: pop_types::FFI_NULL_POINTER_ERROR_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("null pointer error");
    let required_pointer = types
        .intern(SemanticType::Builtin {
            definition: pop_foundation::BuiltinTypeId::from_raw(100),
            arguments: vec![pointer, null_pointer_error],
        })
        .expect("required pointer result");
    let text = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{pointer}) -> (t{boolean}) effects[]\n  b0(v0:t{pointer}):\n    v1:t{optional_pointer} = ffiPointerToOptional v0\n    v2:t{read_only_pointer} = ffiPointerReadOnly v0\n    v3:t{optional_read_only_pointer} = ffiPointerToOptional v2\n    v4:t{optional_read_only_pointer} = ffiPointerNone\n    v5:t{boolean} = ffiPointerIsPresent v1\n    v6:t{boolean} = ffiPointerIsPresent v3\n    v7:t{boolean} = ffiPointerIsPresent v4\n    v8:t{boolean} = booleanNot v7\n    v9:t{boolean} = booleanAnd v5 v6\n    v10:t{boolean} = booleanAnd v9 v8\n    v11:t{required_pointer} = ffiPointerRequire v1 result bt100 success resultCase#0 failure resultCase#1\n    return (v10)\n",
        pointer = pointer.raw(),
        boolean = boolean.raw(),
        optional_pointer = optional_pointer.raw(),
        read_only_pointer = read_only_pointer.raw(),
        optional_read_only_pointer = optional_read_only_pointer.raw(),
        required_pointer = required_pointer.raw(),
    );
    let mir = parse_mir_dump(&text).expect("pointer MIR");
    let llvm = lower_mir_to_llvm_ir(&mir, &types, &target(), LlvmLoweringOptions::default())
        .expect("LLVM lowering");
    let text = llvm.to_string();

    assert!(text.contains("%v1 = select i1 true, i64 %v0, i64 zeroinitializer"));
    assert!(text.contains("%v4 = select i1 true, i64 zeroinitializer, i64 zeroinitializer"));
    assert!(text.contains("%v5 = icmp ne i64 %v1, zeroinitializer"));
    assert!(text.contains("%v11_present = icmp ne i64 %v1, 0"));
    assert!(text.contains("%v11_case = select i1 %v11_present, i64 0, i64 1"));
}
