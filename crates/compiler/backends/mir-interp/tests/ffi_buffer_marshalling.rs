use pop_backend_mir_interp::{MirInterpreter, MirValue};
use pop_foundation::{BuiltinTypeId, FieldId, SymbolId, TypeId};
use pop_mir::{
    MirFfiLayout, MirFfiLayoutCatalog, MirFfiLayoutField, MirFfiValueClass, parse_mir_dump,
};
use pop_runtime_interface::{FfiAbiLayoutId, ForeignAddress};
use pop_target::TargetSpec;
use pop_types::{
    FFI_BUFFER_TYPE_ID, FFI_HANDLE_TYPE_ID, FFI_POINTER_TYPE_ID, FloatValue, IntegerKind,
    IntegerValue, SemanticType, TypeArena,
};

fn layout(raw: u64) -> FfiAbiLayoutId {
    FfiAbiLayoutId::new(raw).expect("nonzero layout")
}

fn field(raw: u32, index: u32, child: u64, offset: u64) -> MirFfiLayoutField {
    MirFfiLayoutField::new(FieldId::from_raw(raw), index, layout(child), offset)
}

fn entry(
    raw: u64,
    element: TypeId,
    size: u64,
    alignment: u64,
    class: MirFfiValueClass,
) -> MirFfiLayout {
    MirFfiLayout::new(layout(raw), element, size, alignment, class)
}

#[test]
fn interpreter_round_trips_all_scalar_ffi_buffer_value_classes() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let float = types.source_type("Float64").expect("Float64");
    let pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("pointer");
    let function = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(202),
            arguments: vec![integer],
        })
        .expect("function pointer");
    let handle = types
        .intern(SemanticType::Builtin {
            definition: FFI_HANDLE_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("handle");
    let c_int = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(215),
            arguments: Vec::new(),
        })
        .expect("C int");

    let cases = [
        (
            integer,
            MirFfiValueClass::Integer,
            8,
            8,
            MirValue::Integer(IntegerValue::parse_decimal("-42", IntegerKind::Int64).unwrap()),
        ),
        (
            float,
            MirFfiValueClass::Float,
            8,
            8,
            MirValue::Float(FloatValue::Float64(12.5_f64.to_bits())),
        ),
        (
            pointer,
            MirFfiValueClass::Pointer,
            8,
            8,
            MirValue::FfiPointer(ForeignAddress::new(0x1234).unwrap()),
        ),
        (
            function,
            MirFfiValueClass::FunctionPointer,
            8,
            8,
            MirValue::FfiFunction(0x5678),
        ),
        (
            handle,
            MirFfiValueClass::Handle,
            8,
            8,
            MirValue::FfiHandle(91),
        ),
        (
            c_int,
            MirFfiValueClass::Integer,
            4,
            4,
            integer_value("-17", IntegerKind::Int32),
        ),
    ];
    for (element, class, size, alignment, value) in cases {
        let output = execute_round_trip(
            &mut types,
            element,
            vec![entry(1, element, size, alignment, class)],
            layout(1),
            "",
            value.clone(),
        );
        assert_eq!(output, value);
    }
}

#[test]
fn interpreter_marshals_nested_layout_records_by_field_plan() {
    let mut types = TypeArena::new();
    let integer32 = types.source_type("Int32").expect("Int32");
    let integer64 = types.source_type("Int64").expect("Int64");
    let inner = types
        .intern(SemanticType::Record(vec![
            ("low".to_owned(), integer32),
            ("high".to_owned(), integer32),
        ]))
        .expect("inner record");
    let outer = types
        .intern(SemanticType::Record(vec![
            ("pair".to_owned(), inner),
            ("tail".to_owned(), integer64),
        ]))
        .expect("outer record");
    let entries = vec![
        entry(1, integer32, 4, 4, MirFfiValueClass::Integer),
        entry(
            2,
            inner,
            8,
            4,
            MirFfiValueClass::Record(vec![field(11, 0, 1, 0), field(12, 1, 1, 4)]),
        ),
        entry(3, integer64, 8, 8, MirFfiValueClass::Integer),
        entry(
            4,
            outer,
            16,
            8,
            MirFfiValueClass::Record(vec![field(21, 0, 2, 0), field(22, 1, 3, 8)]),
        ),
    ];
    let value = MirValue::Record {
        record: SymbolId::from_raw(11),
        fields: vec![
            (
                FieldId::from_raw(21),
                MirValue::Record {
                    record: SymbolId::from_raw(10),
                    fields: vec![
                        (
                            FieldId::from_raw(11),
                            integer_value("7", IntegerKind::Int32),
                        ),
                        (
                            FieldId::from_raw(12),
                            integer_value("9", IntegerKind::Int32),
                        ),
                    ],
                },
            ),
            (
                FieldId::from_raw(22),
                integer_value("123", IntegerKind::Int64),
            ),
        ],
    };
    let declarations = format!(
        "type.record s10 t{} fields field#11:t{},field#12:t{}\ntype.record s11 t{} fields field#21:t{},field#22:t{}\n",
        inner.raw(),
        integer32.raw(),
        integer32.raw(),
        outer.raw(),
        inner.raw(),
        integer64.raw(),
    );
    assert_eq!(
        execute_round_trip(
            &mut types,
            outer,
            entries,
            layout(4),
            &declarations,
            value.clone(),
        ),
        value
    );
}

fn integer_value(text: &str, kind: IntegerKind) -> MirValue {
    MirValue::Integer(IntegerValue::parse_decimal(text, kind).unwrap())
}

fn execute_round_trip(
    types: &mut TypeArena,
    element: TypeId,
    entries: Vec<MirFfiLayout>,
    root_layout: FfiAbiLayoutId,
    declarations: &str,
    value: MirValue,
) -> MirValue {
    let size = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(221),
            arguments: Vec::new(),
        })
        .expect("size");
    let buffer = types
        .intern(SemanticType::Builtin {
            definition: FFI_BUFFER_TYPE_ID,
            arguments: vec![element],
        })
        .expect("buffer");
    let allocation_error = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(209),
            arguments: Vec::new(),
        })
        .expect("allocation error");
    let result = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(100),
            arguments: vec![buffer, allocation_error],
        })
        .expect("result");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let target = TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("target");
    let catalog = MirFfiLayoutCatalog::new(&target, entries, types).expect("catalog");
    let root = catalog.get(root_layout).expect("root layout");
    let text = format!(
        "mir bubble b0 namespace n0\ndependencies\n{declarations}function s0 f0(t{size}, t{size}, t{element}) -> (t{element}) effects[Allocates,MayTrap,GcSafePoint,Roots]\n  b0(v0:t{size}, v1:t{size}, v2:t{element}):\n    do v3 gcSafePoint sp0 roots ()\n    v4:t{result} = ffiBufferOpen v0 element t{element} layout#{layout} size {element_size} align {alignment} result bt100 success resultCase#0 failure resultCase#1\n    v5:t{boolean} = resultIsOk bt100 v4\n    condBranch v5 b1 b2\n  b1():\n    v6:t{buffer} = resultGetOk bt100 v4\n    do v7 ffiBufferWrite v6 v1 v2 layout#{layout}\n    v8:t{element} = ffiBufferRead v6 v1 layout#{layout}\n    do v9 ffiBufferClose v6\n    return (v8)\n  b2():\n    v10:t{allocation_error} = resultGetError bt100 v4\n    return (v2)\n",
        layout = root_layout.raw(),
        element_size = root.size(),
        alignment = root.alignment(),
        size = size.raw(),
        element = element.raw(),
        result = result.raw(),
        boolean = boolean.raw(),
        buffer = buffer.raw(),
        allocation_error = allocation_error.raw(),
    );
    let mir = parse_mir_dump(&text)
        .expect("MIR")
        .with_ffi_layouts(catalog);
    let interpreter = MirInterpreter::new(&mir, types).expect("verified MIR");
    interpreter
        .call(
            SymbolId::from_raw(0),
            &[
                integer_value("1", IntegerKind::UInt64),
                integer_value("1", IntegerKind::UInt64),
                value,
            ],
        )
        .expect("round trip")
        .into_iter()
        .next()
        .expect("one result")
}
