use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{
    AllocationSiteId, BubbleId, BuiltinTypeId, FileId, LifetimeId, ModuleId, NamespaceId,
};
use pop_mir::{
    MirVerificationError, MirViewBoundaryProof, MirViewKind, MirViewLender, MirViewRangeUnit,
    MirViewTrap, lower_hir_bubble, optimize_mir, parse_mir_dump, verify_mir_bubble,
};
use pop_source::SourceFile;
use pop_types::{BYTES_VIEW_TYPE_ID, SemanticType, TEXT_VIEW_TYPE_ID, TypeArena};

fn view_types() -> TypeArena {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let byte = types.source_type("Byte").expect("Byte");
    let string = types.source_type("String").expect("String");
    let bytes = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(0),
            arguments: Vec::new(),
        })
        .expect("Bytes");
    let bytes_view = types
        .intern(SemanticType::Builtin {
            definition: BYTES_VIEW_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("Bytes.View");
    let text_view = types
        .intern(SemanticType::Builtin {
            definition: TEXT_VIEW_TYPE_ID,
            arguments: Vec::new(),
        })
        .expect("Text.View");
    let _ = types.optional(byte).expect("Byte?");
    let _ = (integer, string, bytes, bytes_view, text_view);
    types
}

fn bytes_view_text(types: &TypeArena) -> String {
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
    format!(
        concat!(
            "mir bubble b0 namespace n0\n",
            "dependencies\n",
            "function s0 f0(t{bytes}, t{integer}, t{integer}) -> (t{bytes}, t{integer}, t{optional_byte}) effects[Allocates,MayTrap,GcSafePoint,Roots]\n",
            "  b0(v0:t{bytes}, v1:t{integer}, v2:t{integer}):\n",
            "    v3:t{view} = viewCreate bytes v0 lender parameter#0 unit bytes boundary none lifetime#1\n",
            "    v4:t{view} = viewSlice bytes v3 v1 v2 lender parameter#0 unit bytes boundary none parent lifetime#1 lifetime#2 trap BoundsViolation\n",
            "    v5:t{integer} = viewLength bytes v4\n",
            "    v6:t{optional_byte} = viewGetByte v4 v1\n",
            "    do v7 gcSafePoint sp0 roots (v0)\n",
            "    v8:t{bytes} = viewMaterialize bytes v4 allocation#7\n",
            "    do v9 viewEnd lifetime#2\n",
            "    do v10 viewEnd lifetime#1\n",
            "    return (v8,v5,v6)\n",
        ),
        bytes = bytes.raw(),
        integer = integer.raw(),
        optional_byte = optional_byte.raw(),
        view = view.raw(),
    )
}

#[test]
fn canonical_view_operations_round_trip_and_survive_optimization() {
    let types = view_types();
    let text = bytes_view_text(&types);
    let bubble = parse_mir_dump(&text).expect("view MIR");
    assert_eq!(verify_mir_bubble(&bubble, &types), Ok(()));

    let dump = bubble.dump();
    assert!(dump.contains("viewCreate bytes v0 lender parameter#0"));
    assert!(dump.contains("parent lifetime#1 lifetime#2 trap BoundsViolation"));
    assert!(dump.contains("viewMaterialize bytes v4 allocation#7"));
    assert_eq!(parse_mir_dump(&dump).expect("view round trip"), bubble);

    let optimized = optimize_mir(bubble, &types).expect("verified optimized view MIR");
    assert_eq!(verify_mir_bubble(&optimized, &types), Ok(()));
    let optimized_dump = optimized.dump();
    assert!(optimized_dump.contains("lifetime#1"));
    assert!(optimized_dump.contains("lifetime#2"));
    assert!(optimized_dump.contains("allocation#7"));
}

#[test]
fn verifier_rejects_corrupt_view_contracts_and_escape() {
    let types = view_types();
    let text = bytes_view_text(&types);
    let corruptions = [
        text.replace("viewCreate bytes", "viewCreate text"),
        text.replace("unit bytes", "unit scalars"),
        text.replace("boundary none", "boundary utf8"),
        text.replace("parent lifetime#1", "parent lifetime#9"),
        text.replace("lifetime#2 trap", "lifetime#1 trap"),
        text.replace("    do v9 viewEnd lifetime#2\n", ""),
        text.replace("roots (v0)", "roots ()"),
        text.replace("return (v8,v5,v6)", "return (v4,v5,v6)"),
    ];
    for corrupt in corruptions {
        let bubble = parse_mir_dump(&corrupt).expect("structurally valid corrupt view MIR");
        assert!(
            verify_mir_bubble(&bubble, &types).is_err(),
            "corrupt view MIR was accepted:\n{corrupt}"
        );
    }
    assert!(
        parse_mir_dump(&text.replace(" trap BoundsViolation", " trap none")).is_err(),
        "the closed view trap vocabulary accepted an invented fallback"
    );
}

#[test]
fn view_contract_vocabulary_is_closed_and_typed() {
    assert_eq!(MirViewKind::Bytes.range_unit(), MirViewRangeUnit::Bytes);
    assert_eq!(
        MirViewKind::Text.boundary_proof(),
        MirViewBoundaryProof::Utf8Scalar
    );
    assert_eq!(MirViewTrap::BoundsViolation.to_string(), "BoundsViolation");
    assert_eq!(
        MirViewLender::Parameter { index: 3 }.parameter_index(),
        Some(3)
    );
    assert_ne!(LifetimeId::from_raw(4), LifetimeId::from_raw(5));
    assert_ne!(AllocationSiteId::from_raw(7), AllocationSiteId::from_raw(8));
    let _ = MirVerificationError::InvalidViewLifetime {
        lifetime: LifetimeId::from_raw(1),
    };
}

#[test]
fn direct_alias_call_contract_round_trips_and_rejects_corruption() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/views.pop",
        "namespace Main\n\
         private function middle(view: Bytes.View): Bytes.View\n\
             return Bytes.slice(view, 2, 2)\n\
         end\n\
         public function copyMiddle(bytes: Bytes): Bytes\n\
             local whole = Bytes.view(bytes)\n\
             return Bytes.toBytes(middle(whole))\n\
         end\n",
    )
    .expect("view source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(front_end.diagnostics().is_empty());
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types())
        .expect("call-borrow MIR");
    let dump = mir.dump();
    assert!(
        dump.contains("lifetimeSummary(v1;parameters=DoesNotRetain;results=ReturnsAlias#0) viewResult(bytes,source#0,lifetime#"),
        "{dump}"
    );
    assert_eq!(
        parse_mir_dump(&dump)
            .expect("call-borrow round trip")
            .dump(),
        dump
    );

    let corrupt = dump.replacen("viewResult(bytes,source#0", "viewResult(bytes,source#9", 1);
    let corrupt = parse_mir_dump(&corrupt).expect("structurally valid corrupt call borrow");
    assert!(verify_mir_bubble(&corrupt, front_end.types()).is_err());
}
