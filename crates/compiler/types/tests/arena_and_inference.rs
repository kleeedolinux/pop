use pop_types::{
    InferenceContext, InferenceError, InferenceType, IntegerKind, PrimitiveType, SemanticType,
    TypeArena,
};

#[test]
fn primitive_aliases_share_canonical_type_ids() {
    let arena = TypeArena::new();

    assert_eq!(arena.source_type("Int"), arena.source_type("Int64"));
    assert_eq!(arena.source_type("Float"), arena.source_type("Float64"));
    assert_eq!(arena.source_type("Byte"), arena.source_type("UInt8"));
    assert_eq!(
        arena.source_type("Int32").map(|id| arena.get(id)),
        Some(Some(&SemanticType::Primitive(PrimitiveType::Integer(
            IntegerKind::Int32
        ))))
    );
    assert_eq!(arena.source_type("Any"), None);
    assert_eq!(arena.source_type("Dynamic"), None);
}

#[test]
fn unions_are_flattened_sorted_deduplicated_and_drop_never() {
    let mut arena = TypeArena::new();
    let string = arena.source_type("String").expect("String");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let never = arena.source_type("Never").expect("Never");
    let inner = arena.union([string, boolean]).expect("inner union");
    let outer = arena
        .union([never, string, inner, boolean, string])
        .expect("normalized union");

    assert_eq!(outer, inner);
    assert_eq!(
        arena.get(outer),
        Some(&SemanticType::Union(vec![boolean, string]))
    );
}

#[test]
fn optional_normalizes_to_a_union_with_nil() {
    let mut arena = TypeArena::new();
    let string = arena.source_type("String").expect("String");
    let nil = arena.source_type("nil").expect("nil");
    let optional = arena.optional(string).expect("optional");

    assert_eq!(
        arena.get(optional),
        Some(&SemanticType::Union(vec![nil, string]))
    );
    assert_eq!(
        arena.optional(optional).expect("idempotent optional"),
        optional
    );
}

#[test]
fn unsolved_inference_is_an_error_not_a_runtime_type() {
    let mut inference = InferenceContext::new();
    let variable = inference.new_variable();

    assert_eq!(
        inference.resolve(variable),
        Err(InferenceError::Unsolved(variable))
    );
}

#[test]
fn equality_constraints_propagate_types_and_report_conflicts() {
    let arena = TypeArena::new();
    let string = arena.source_type("String").expect("String");
    let boolean = arena.source_type("Boolean").expect("Boolean");
    let mut inference = InferenceContext::new();
    let first = inference.new_variable();
    let second = inference.new_variable();

    inference
        .constrain_equal(
            InferenceType::Variable(first),
            InferenceType::Variable(second),
        )
        .expect("variable equality");
    inference
        .constrain_equal(InferenceType::Variable(first), InferenceType::Known(string))
        .expect("bind String");
    assert_eq!(inference.resolve(second), Ok(string));
    assert_eq!(
        inference.constrain_equal(
            InferenceType::Variable(second),
            InferenceType::Known(boolean)
        ),
        Err(InferenceError::Conflict {
            expected: string,
            found: boolean,
        })
    );
}
