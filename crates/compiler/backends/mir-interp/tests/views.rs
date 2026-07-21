use pop_backend_mir_interp::{MirInterpreter, MirValue};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, BuiltinTypeId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_mir::{lower_hir_bubble, parse_mir_dump, verify_mir_bubble};
use pop_runtime_collector::GenerationalRuntime;
use pop_runtime_interface::{
    ArrayAllocationRequest, GarbageCollectorContract, ManagedReference, ObjectAllocationRequest,
    RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, SafePointOutcome,
    TableAllocationRequest, TrapKind, WriteBarrier,
};
use pop_source::SourceFile;
use pop_types::{
    BYTES_VIEW_TYPE_ID, IntegerKind, IntegerValue, SemanticType, TEXT_VIEW_TYPE_ID, TypeArena,
};

struct RelocatingRuntime(GenerationalRuntime);

impl RuntimeAdapter for RelocatingRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        self.0.contract()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.0.allocate_object(request)
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.0.allocate_array(request)
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.0.allocate_table(request)
    }

    fn allocate_immutable_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.0.allocate_immutable_bytes(bytes)
    }

    fn immutable_bytes_length(&self, bytes: ManagedReference) -> Result<u64, RuntimeFailure> {
        self.0.immutable_bytes_length(bytes)
    }

    fn immutable_bytes_read(
        &self,
        bytes: ManagedReference,
        offset: u64,
        target: &mut [u8],
    ) -> Result<(), RuntimeFailure> {
        self.0.immutable_bytes_read(bytes, offset, target)
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.0.retain_root(reference)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.0.release_root(root)
    }

    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        self.0.request_minor_collection();
        self.0.safe_point(roots)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.0.write_barrier(barrier)
    }
}

fn types() -> TypeArena {
    let mut types = TypeArena::new();
    let byte = types.source_type("Byte").expect("Byte");
    types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(0),
            arguments: Vec::new(),
        })
        .expect("Bytes");
    types
        .intern(SemanticType::Builtin {
            definition: BYTES_VIEW_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("Bytes.View");
    types
        .intern(SemanticType::Builtin {
            definition: TEXT_VIEW_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("Text.View");
    types.optional(byte).expect("Byte?");
    types
}

fn int_value(value: i64) -> MirValue {
    MirValue::Integer(
        IntegerValue::parse_decimal(&value.to_string(), IntegerKind::Int64).expect("Int"),
    )
}

fn is_bounds_violation(
    result: Result<Vec<MirValue>, pop_backend_mir_interp::ExecutionError>,
) -> bool {
    matches!(
        result,
        Err(pop_backend_mir_interp::ExecutionError::Runtime(
            RuntimeFailure::Trap(trap)
        )) if trap.kind() == TrapKind::BoundsViolation
    )
}

#[test]
fn bytes_views_re_read_relocated_lenders_for_slice_get_length_and_materialize() {
    let types = types();
    let integer = types.source_type("Int").expect("Int");
    let byte = types.source_type("Byte").expect("Byte");
    let nil = types.source_type("nil").expect("nil");
    let bytes = types
        .find(&SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(0),
            arguments: Vec::new(),
        })
        .expect("Bytes");
    let view = types
        .find(&SemanticType::Builtin {
            definition: BYTES_VIEW_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("Bytes.View");
    let optional_byte = types
        .find(&SemanticType::Union(vec![nil, byte]))
        .expect("Byte?");
    let text = format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{bytes}, t{integer}, t{integer}) -> (t{bytes}, t{integer}, t{optional_byte}) effects[Allocates,MayTrap,GcSafePoint,Roots]\n",
            "  b0(v0:t{bytes}, v1:t{integer}, v2:t{integer}):\n",
            "    v3:t{view} = viewCreate bytes v0 lender parameter#0 unit bytes boundary none lifetime#1\n",
            "    do v4 gcSafePoint sp0 roots (v0)\n",
            "    v5:t{view} = viewSlice bytes v3 v1 v2 lender parameter#0 unit bytes boundary none parent lifetime#1 lifetime#2 trap BoundsViolation\n",
            "    v6:t{integer} = viewLength bytes v5\n",
            "    v7:t{optional_byte} = viewGetByte v5 v1\n",
            "    do v8 gcSafePoint sp1 roots (v0)\n",
            "    v9:t{bytes} = viewMaterialize bytes v5 allocation#7\n",
            "    do v10 viewEnd lifetime#2\n",
            "    do v11 viewEnd lifetime#1\n",
            "    return (v9,v6,v7)\n",
        ),
        bytes = bytes.raw(),
        integer = integer.raw(),
        optional_byte = optional_byte.raw(),
        view = view.raw(),
    );
    let mir = parse_mir_dump(&text).expect("bytes view MIR");
    assert_eq!(verify_mir_bubble(&mir, &types), Ok(()));
    let mut runtime = RelocatingRuntime(GenerationalRuntime::new());
    let owner = runtime
        .allocate_immutable_bytes(&[10, 20, 30, 40])
        .expect("Bytes owner");
    let interpreter = MirInterpreter::with_runtime(&mir, &types, runtime).expect("interpreter");

    let returned = interpreter
        .call(
            SymbolId::from_raw(0),
            &[MirValue::Bytes(owner), int_value(2), int_value(2)],
        )
        .expect("relocation-safe view execution");
    assert_eq!(returned[1], int_value(2));
    assert_eq!(
        returned[2],
        MirValue::Integer(IntegerValue::parse_decimal("30", IntegerKind::UInt8).expect("Byte"))
    );
    let MirValue::Bytes(copy) = returned[0] else {
        panic!("materialized Bytes result");
    };
    let runtime = interpreter.runtime();
    let mut payload = [0_u8; 2];
    runtime
        .immutable_bytes_read(copy, 0, &mut payload)
        .expect("materialized payload");
    assert_eq!(payload, [20, 30]);
}

#[test]
fn bytes_views_enforce_one_based_checked_ranges_and_optional_access() {
    let types = types();
    let integer = types.source_type("Int").expect("Int");
    let byte = types.source_type("Byte").expect("Byte");
    let nil = types.source_type("nil").expect("nil");
    let bytes = types
        .find(&SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(0),
            arguments: Vec::new(),
        })
        .expect("Bytes");
    let view = types
        .find(&SemanticType::Builtin {
            definition: BYTES_VIEW_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("Bytes.View");
    let optional_byte = types
        .find(&SemanticType::Union(vec![nil, byte]))
        .expect("Byte?");
    let text = format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{bytes}, t{integer}, t{integer}) -> (t{integer}, t{optional_byte}) effects[MayTrap]\n",
            "  b0(v0:t{bytes}, v1:t{integer}, v2:t{integer}):\n",
            "    v3:t{view} = viewCreate bytes v0 lender parameter#0 unit bytes boundary none lifetime#1\n",
            "    v4:t{view} = viewSlice bytes v3 v1 v2 lender parameter#0 unit bytes boundary none parent lifetime#1 lifetime#2 trap BoundsViolation\n",
            "    v5:t{integer} = viewLength bytes v4\n",
            "    v6:t{optional_byte} = viewGetByte v4 v1\n",
            "    do v7 viewEnd lifetime#2\n",
            "    do v8 viewEnd lifetime#1\n",
            "    return (v5,v6)\n",
        ),
        bytes = bytes.raw(),
        integer = integer.raw(),
        optional_byte = optional_byte.raw(),
        view = view.raw(),
    );
    let mir = parse_mir_dump(&text).expect("bounds view MIR");
    let mut runtime = GenerationalRuntime::new();
    let owner = runtime
        .allocate_immutable_bytes(&[10, 20, 30, 40])
        .expect("Bytes owner");
    let interpreter = MirInterpreter::with_runtime(&mir, &types, runtime).expect("interpreter");

    assert_eq!(
        interpreter
            .call(
                SymbolId::from_raw(0),
                &[MirValue::Bytes(owner), int_value(5), int_value(0)],
            )
            .expect("zero length at owner boundary"),
        vec![int_value(0), MirValue::Nil],
    );
    for (start, length) in [(0, 0), (6, 0), (4, 2), (1, -1), (2, i64::MAX)] {
        assert!(
            is_bounds_violation(interpreter.call(
                SymbolId::from_raw(0),
                &[MirValue::Bytes(owner), int_value(start), int_value(length)],
            )),
            "invalid range start={start} length={length} did not trap",
        );
    }
}

#[test]
fn text_views_slice_unicode_scalars_without_splitting_utf8() {
    let types = types();
    let integer = types.source_type("Int").expect("Int");
    let string = types.source_type("String").expect("String");
    let view = types
        .find(&SemanticType::Builtin {
            definition: TEXT_VIEW_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("Text.View");
    let text = format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{string}, t{integer}, t{integer}) -> (t{string}, t{integer}) effects[Allocates,MayTrap,GcSafePoint,Roots]\n",
            "  b0(v0:t{string}, v1:t{integer}, v2:t{integer}):\n",
            "    v3:t{view} = viewCreate text v0 lender parameter#0 unit scalars boundary utf8 lifetime#1\n",
            "    v4:t{view} = viewSlice text v3 v1 v2 lender parameter#0 unit scalars boundary utf8 parent lifetime#1 lifetime#2 trap BoundsViolation\n",
            "    v5:t{integer} = viewLength text v4\n",
            "    do v6 gcSafePoint sp0 roots (v0)\n",
            "    v7:t{string} = viewMaterialize text v4 allocation#9\n",
            "    do v8 viewEnd lifetime#2\n",
            "    do v9 viewEnd lifetime#1\n",
            "    return (v7,v5)\n",
        ),
        string = string.raw(),
        integer = integer.raw(),
        view = view.raw(),
    );
    let mir = parse_mir_dump(&text).expect("text view MIR");
    let interpreter = MirInterpreter::new(&mir, &types).expect("interpreter");

    assert_eq!(
        interpreter
            .call(
                SymbolId::from_raw(0),
                &[
                    MirValue::String("aé🦀z".to_owned()),
                    int_value(2),
                    int_value(2)
                ],
            )
            .expect("UTF-8 scalar slice"),
        vec![MirValue::String("é🦀".to_owned()), int_value(2)]
    );
}

#[test]
fn view_parameters_and_alias_results_rebind_across_direct_calls() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/views.pop",
        "namespace Main\n\
         private function middle(view: Bytes.View): Bytes.View\n\
             return Bytes.slice(view, 2, 2)\n\
         end\n\
         public function copyMiddle(bytes: Bytes): Bytes\n\
             local whole = Bytes.view(bytes)\n\
             local selected = middle(whole)\n\
             return Bytes.toBytes(selected)\n\
         end\n",
    )
    .expect("view source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let hir = front_end.hir().expect("view HIR");
    let entry = hir
        .functions()
        .iter()
        .find(|function| function.name() == "copyMiddle")
        .expect("copyMiddle")
        .symbol();
    let mir = lower_hir_bubble(hir, front_end.types()).expect("view MIR");
    let mut runtime = GenerationalRuntime::new();
    let owner = runtime
        .allocate_immutable_bytes(&[10, 20, 30, 40])
        .expect("Bytes owner");
    let interpreter =
        MirInterpreter::with_runtime(&mir, front_end.types(), runtime).expect("view interpreter");

    let returned = interpreter
        .call(entry, &[MirValue::Bytes(owner)])
        .expect("cross-call view alias");
    let [MirValue::Bytes(copy)] = returned.as_slice() else {
        panic!("owned Bytes copy result");
    };
    let mut payload = [0_u8; 2];
    interpreter
        .runtime()
        .immutable_bytes_read(*copy, 0, &mut payload)
        .expect("copied view payload");
    assert_eq!(payload, [20, 30]);
}
