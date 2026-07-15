use pop_foundation::{FieldId, TypeId};
use pop_mir::{
    MirFfiLayout, MirFfiLayoutCatalog, MirFfiLayoutError, MirFfiLayoutField, MirFfiValueClass,
};
use pop_runtime_interface::FfiAbiLayoutId;
use pop_types::{FFI_POINTER_TYPE_ID, SemanticType, TypeArena};

fn layout(raw: u64) -> FfiAbiLayoutId {
    FfiAbiLayoutId::new(raw).expect("nonzero layout")
}

fn field(raw: u32, source_index: u32, layout_id: u64, offset: u64) -> MirFfiLayoutField {
    MirFfiLayoutField::new(
        FieldId::from_raw(raw),
        source_index,
        layout(layout_id),
        offset,
    )
}

fn entry(
    raw: u64,
    element: TypeId,
    size: u64,
    alignment: u64,
    value_class: MirFfiValueClass,
) -> MirFfiLayout {
    MirFfiLayout::new(layout(raw), element, size, alignment, value_class)
}

#[test]
fn catalog_validates_and_orders_nested_target_layouts() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("pointer");
    let record = types
        .intern(SemanticType::Record(vec![
            ("count".to_owned(), integer),
            ("data".to_owned(), pointer),
        ]))
        .expect("record");

    let catalog = MirFfiLayoutCatalog::new(
        "x86_64-unknown-linux-gnu",
        vec![
            entry(
                3,
                record,
                16,
                8,
                MirFfiValueClass::Record(vec![field(1, 0, 1, 0), field(2, 1, 2, 8)]),
            ),
            entry(2, pointer, 8, 8, MirFfiValueClass::Pointer),
            entry(1, integer, 8, 8, MirFfiValueClass::Integer),
        ],
        &types,
    )
    .expect("valid catalog");

    assert_eq!(catalog.target(), "x86_64-unknown-linux-gnu");
    assert_eq!(
        catalog
            .entries()
            .iter()
            .map(|entry| entry.id().raw())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        catalog.get(layout(3)).map(MirFfiLayout::element),
        Some(record)
    );
}

#[test]
fn catalog_rejects_duplicate_invalid_geometry_and_managed_types() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(SemanticType::Array(integer))
        .expect("managed array");

    assert_eq!(
        MirFfiLayoutCatalog::new(
            "target",
            vec![
                entry(1, integer, 8, 8, MirFfiValueClass::Integer),
                entry(1, integer, 8, 8, MirFfiValueClass::Integer),
            ],
            &types,
        ),
        Err(MirFfiLayoutError::DuplicateLayout(layout(1)))
    );
    assert_eq!(
        MirFfiLayoutCatalog::new(
            "target",
            vec![entry(1, integer, 8, 3, MirFfiValueClass::Integer)],
            &types,
        ),
        Err(MirFfiLayoutError::InvalidGeometry(layout(1)))
    );
    assert_eq!(
        MirFfiLayoutCatalog::new(
            "target",
            vec![entry(1, array, 8, 8, MirFfiValueClass::Pointer)],
            &types,
        ),
        Err(MirFfiLayoutError::TypeClassMismatch(layout(1)))
    );
}

#[test]
fn catalog_rejects_overlapping_and_recursive_record_plans() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let integer32 = types.source_type("Int32").expect("Int32");
    let pair = types
        .intern(SemanticType::Record(vec![
            ("left".to_owned(), integer),
            ("right".to_owned(), integer),
        ]))
        .expect("pair");
    let triple = types
        .intern(SemanticType::Record(vec![
            ("first".to_owned(), integer32),
            ("second".to_owned(), integer32),
            ("third".to_owned(), integer32),
        ]))
        .expect("triple");
    let triple_pair = types
        .intern(SemanticType::Record(vec![
            ("left".to_owned(), triple),
            ("right".to_owned(), triple),
        ]))
        .expect("triple pair");

    assert_eq!(
        MirFfiLayoutCatalog::new(
            "target",
            vec![
                entry(1, integer32, 4, 4, MirFfiValueClass::Integer),
                entry(
                    2,
                    triple,
                    12,
                    4,
                    MirFfiValueClass::Record(vec![
                        field(1, 0, 1, 0),
                        field(2, 1, 1, 4),
                        field(3, 2, 1, 8),
                    ]),
                ),
                entry(
                    3,
                    triple_pair,
                    24,
                    4,
                    MirFfiValueClass::Record(vec![field(4, 0, 2, 0), field(5, 1, 2, 8)]),
                ),
            ],
            &types,
        ),
        Err(MirFfiLayoutError::OverlappingFields(layout(3)))
    );

    let first = types
        .intern(SemanticType::Record(vec![("next".to_owned(), pair)]))
        .expect("first");
    let second = types
        .intern(SemanticType::Record(vec![("next".to_owned(), first)]))
        .expect("second");
    assert_eq!(
        MirFfiLayoutCatalog::new(
            "target",
            vec![
                entry(
                    10,
                    first,
                    16,
                    8,
                    MirFfiValueClass::Record(vec![field(10, 0, 11, 0)]),
                ),
                entry(
                    11,
                    second,
                    16,
                    8,
                    MirFfiValueClass::Record(vec![field(11, 0, 10, 0)]),
                ),
            ],
            &types,
        ),
        Err(MirFfiLayoutError::RecursiveByValueLayout(layout(10)))
    );
}
