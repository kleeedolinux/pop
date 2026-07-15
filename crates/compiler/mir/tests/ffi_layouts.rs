use pop_driver::artifact_sha256_hex;
use pop_foundation::{BuiltinTypeId, FieldId, TypeId};
use pop_mir::{
    MirFfiLayout, MirFfiLayoutCatalog, MirFfiLayoutError, MirFfiLayoutField, MirFfiValueClass,
    parse_mir_dump,
};
use pop_runtime_interface::FfiAbiLayoutId;
use pop_target::TargetSpec;
use pop_types::{FFI_POINTER_TYPE_ID, ForeignAbi, SemanticType, TypeArena};

fn layout(raw: u64) -> FfiAbiLayoutId {
    FfiAbiLayoutId::new(raw).expect("nonzero layout")
}

fn target() -> TargetSpec {
    TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target")
}

fn build_catalog(
    target: &TargetSpec,
    entries: Vec<MirFfiLayout>,
    types: &TypeArena,
) -> Result<MirFfiLayoutCatalog, MirFfiLayoutError> {
    MirFfiLayoutCatalog::new(target, entries, types, artifact_sha256_hex)
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
fn catalog_derives_scalar_identity_from_canonical_descriptor_bytes() {
    let types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let catalog = build_catalog(
        &target(),
        vec![entry(99, integer, 8, 8, MirFfiValueClass::Integer)],
        &types,
    )
    .expect("canonical scalar catalog");
    let [scalar] = catalog.entries() else {
        panic!("one scalar layout");
    };

    assert_eq!(
        scalar.descriptor(),
        "{\"schemaVersion\":1,\"target\":\"x86_64-unknown-linux-gnu\",\"abi\":\"C\",\"abiType\":\"Int64\",\"size\":8,\"alignment\":8}"
    );
    assert_eq!(
        scalar.fingerprint(),
        "ebec5c4b572171b0c9b360015cf117534aeaaafc40687f55d7c1de482bbcd04f"
    );
    assert_eq!(scalar.id().raw(), 17_000_064_172_070_891_952);

    let system = build_catalog(
        &target(),
        vec![MirFfiLayout::new_for_abi(
            layout(99),
            integer,
            8,
            8,
            MirFfiValueClass::Integer,
            ForeignAbi::System,
        )],
        &types,
    )
    .expect("system ABI catalog");
    assert!(
        system.entries()[0]
            .descriptor()
            .contains("\"abi\":\"System\"")
    );
    assert_ne!(system.entries()[0].id(), scalar.id());
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

    let catalog = build_catalog(
        &target(),
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
    assert!(catalog.entries().is_sorted_by_key(MirFfiLayout::id));
    assert_eq!(
        catalog
            .entries()
            .iter()
            .find(|entry| entry.element() == record)
            .map(MirFfiLayout::element),
        Some(record)
    );
    let record_layout = catalog
        .entries()
        .iter()
        .find(|entry| entry.element() == record)
        .expect("record layout");
    assert_eq!(
        record_layout.descriptor(),
        "{\"schemaVersion\":1,\"target\":\"x86_64-unknown-linux-gnu\",\"abi\":\"C\",\"size\":16,\"alignment\":8,\"fields\":[{\"name\":\"count\",\"abiType\":\"Int64\",\"offset\":0,\"size\":8,\"alignment\":8},{\"name\":\"data\",\"abiType\":\"Ffi.Pointer<Int64>\",\"offset\":8,\"size\":8,\"alignment\":8}]}"
    );
    assert_eq!(
        record_layout.fingerprint(),
        "65f6d02fbbd2412dd70ca436e7aabf8cf9e034af13a1db9415e2867f95c2f98c"
    );
    assert_eq!(record_layout.id().raw(), 7_347_288_745_534_701_869);
}

#[test]
fn catalog_identity_ignores_provisional_keys_and_local_type_allocation_order() {
    let build = |with_noise: bool, keys: [u64; 3]| {
        let mut types = TypeArena::new();
        let integer = types.source_type("Int").expect("Int");
        if with_noise {
            types
                .intern(SemanticType::Array(integer))
                .expect("unrelated type");
        }
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
        build_catalog(
            &target(),
            vec![
                entry(keys[0], integer, 8, 8, MirFfiValueClass::Integer),
                entry(keys[1], pointer, 8, 8, MirFfiValueClass::Pointer),
                entry(
                    keys[2],
                    record,
                    16,
                    8,
                    MirFfiValueClass::Record(vec![
                        field(1, 0, keys[0], 0),
                        field(2, 1, keys[1], 8),
                    ]),
                ),
            ],
            &types,
        )
        .expect("stable catalog")
        .entries()
        .iter()
        .map(|entry| (entry.id(), entry.fingerprint().to_owned()))
        .collect::<Vec<_>>()
    };

    assert_eq!(build(false, [1, 2, 3]), build(true, [91, 37, 82]));
}

#[test]
fn catalog_rejects_invalid_zero_and_colliding_artifact_fingerprints() {
    let types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let float = types.source_type("Float64").expect("Float64");
    let entries = || {
        vec![
            entry(1, integer, 8, 8, MirFfiValueClass::Integer),
            entry(2, float, 8, 8, MirFfiValueClass::Float),
        ]
    };

    assert!(matches!(
        MirFfiLayoutCatalog::new(&target(), entries(), &types, |_| "A".repeat(64)),
        Err(MirFfiLayoutError::InvalidFingerprint(_))
    ));
    assert!(matches!(
        MirFfiLayoutCatalog::new(&target(), entries(), &types, |_| "0".repeat(64)),
        Err(MirFfiLayoutError::ZeroCompactIdentity(_))
    ));
    assert!(matches!(
        MirFfiLayoutCatalog::new(&target(), entries(), &types, |_| "1".repeat(64)),
        Err(MirFfiLayoutError::CompactIdentityCollision(_))
    ));
}

#[test]
fn every_mir_bubble_carries_an_exact_target_catalog() {
    let catalog = MirFfiLayoutCatalog::empty(&target());
    assert_eq!(catalog.target(), "x86_64-unknown-linux-gnu");
    assert!(catalog.entries().is_empty());

    let bubble =
        parse_mir_dump("mir bubble b0 namespace n0\ndependencies\n").expect("minimal MIR bubble");
    assert_eq!(bubble.ffi_layouts().target(), "x86_64-unknown-linux-gnu");
    assert!(bubble.ffi_layouts().entries().is_empty());
}

#[test]
fn catalog_rejects_duplicate_invalid_geometry_and_managed_types() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(SemanticType::Array(integer))
        .expect("managed array");

    assert_eq!(
        build_catalog(
            &target(),
            vec![
                entry(1, integer, 8, 8, MirFfiValueClass::Integer),
                entry(1, integer, 8, 8, MirFfiValueClass::Integer),
            ],
            &types,
        ),
        Err(MirFfiLayoutError::DuplicateLayout(layout(1)))
    );
    assert_eq!(
        build_catalog(
            &target(),
            vec![entry(1, integer, 8, 3, MirFfiValueClass::Integer)],
            &types,
        ),
        Err(MirFfiLayoutError::InvalidGeometry(layout(1)))
    );
    assert_eq!(
        build_catalog(
            &target(),
            vec![entry(1, array, 8, 8, MirFfiValueClass::Pointer)],
            &types,
        ),
        Err(MirFfiLayoutError::TypeClassMismatch(layout(1)))
    );
}

#[test]
fn catalog_rejects_unsupported_targets_and_false_target_geometry() {
    let mut types = TypeArena::new();
    let c_int = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(215),
            arguments: Vec::new(),
        })
        .expect("C int");
    let unsupported =
        TargetSpec::for_triple("bpfel-unknown-none").expect("target without an accepted C ABI");

    assert_eq!(
        MirFfiLayoutCatalog::empty(&unsupported).target(),
        "bpfel-unknown-none"
    );
    assert_eq!(
        build_catalog(
            &unsupported,
            vec![entry(1, c_int, 4, 4, MirFfiValueClass::Integer)],
            &types,
        ),
        Err(MirFfiLayoutError::UnsupportedTarget)
    );
    assert_eq!(
        build_catalog(
            &target(),
            vec![entry(1, c_int, 8, 8, MirFfiValueClass::Integer)],
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
        build_catalog(
            &target(),
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
        build_catalog(
            &target(),
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
