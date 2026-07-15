use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{lower_hir_bubble, optimize_mir, verify_mir_bubble};
use pop_source::SourceFile;

#[test]
fn portable_optimization_folds_constants_and_removes_dead_values_and_blocks() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
         public function optimized(): Int\n\
             local unusedValue = 99\n\
             if true then\n\
                 return 1 + 2\n\
             else\n\
                 return 5\n\
             end\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let original_blocks = construction.functions()[0].blocks().len();
    let optimized = optimize_mir(construction, front_end.types()).expect("verified optimized MIR");

    assert!(verify_mir_bubble(&optimized, front_end.types()).is_ok());
    assert!(optimized.functions()[0].blocks().len() < original_blocks);
    let dump = optimized.dump();
    assert!(dump.contains("const.integer Int64 3"));
    assert!(!dump.contains("const.integer Int64 99"));
    assert!(!dump.contains("integer.checkedAdd"));
    assert!(!dump.contains("condBranch"));
}

#[test]
fn portable_optimization_covers_nested_async_task_bodies() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/nestedAsyncOptimization.pop",
        "namespace Main\n\
         public function makeTask(): Task<Int>\n\
             local operation = async function(): Int\n\
                 local unusedValue = 99\n\
                 if true then\n\
                     return 1 + 2\n\
                 else\n\
                     return 5\n\
                 end\n\
             end\n\
             return operation()\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let original_blocks = construction.nested_functions()[0].blocks().len();
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");

    assert!(verify_mir_bubble(&optimized, front_end.types()).is_ok());
    assert!(optimized.nested_functions()[0].blocks().len() < original_blocks);
    let dump = optimized.dump();
    assert!(dump.contains("const.integer Int64 3"));
    assert!(!dump.contains("const.integer Int64 99"));
    assert!(!dump.contains("integer.checkedAdd"));
}

#[test]
fn portable_optimization_folds_typed_primitive_equality() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/equality.pop",
        "namespace Main\n\
         public function equality(): (Boolean, Boolean, Boolean)\n\
             return (1 == 1, \"pop\" ~= \"lua\", true == false)\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");
    let dump = optimized.dump();

    assert!(!dump.contains("compareEqual"));
    assert!(!dump.contains("compareNotEqual"));
    assert_eq!(dump.matches("const.boolean true").count(), 2);
    assert_eq!(dump.matches("const.boolean false").count(), 1);
}

#[test]
fn portable_optimization_folds_constant_string_composition() {
    // ADR 0041 permits compile-time folding only when every formatted value
    // and concatenated segment is already constant.
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/stringFolding.pop",
        "namespace Main\n\
         public function describe(): String\n\
             return `value={12}, ratio={1.5}, enabled={true}` .. \"!\"\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");
    let dump = optimized.dump();

    assert!(dump.contains("const.string \"value=12, ratio=1.5, enabled=true!\""));
    assert!(!dump.contains("string.format"));
    assert!(!dump.contains("string.concat"));
}

#[test]
fn portable_optimization_summarizes_constant_bounded_integer_reductions() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/countedReduction.pop",
        "namespace Main\n\
         public function countedReduction(): Int\n\
             local index = 1\n\
             local total = 0\n\
             repeat\n\
                 total = total + index\n\
                 index = index + 1\n\
             until index == 50000001\n\
             return total\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");
    let dump = optimized.dump();

    assert!(dump.contains("const.integer Int64 1250000025000000"));
    assert!(!dump.contains("integer.checkedAdd"));
    assert!(!dump.contains("gcSafePoint"));
    assert!(!dump.contains("condBranch"));
    assert!(optimized.functions()[0].blocks().len() <= 2);
}

#[test]
fn portable_optimization_preserves_unbounded_integer_reductions() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/unboundedReduction.pop",
        "namespace Main\n\
         public function unboundedReduction(limit: Int): Int\n\
             local index = 1\n\
             local total = 0\n\
             repeat\n\
                 total = total + index\n\
                 index = index + 1\n\
             until index == limit\n\
             return total\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");
    let dump = optimized.dump();

    assert!(dump.contains("integer.checkedAdd Int64"));
    assert!(dump.contains("gcSafePoint"));
    assert!(dump.contains("condBranch"));
}

#[test]
fn portable_optimization_preserves_zero_result_calls() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/effects.pop",
        "namespace Main\n\
         private function observe(value: Int)\n\
             value\n\
         end\n\
         public function run()\n\
             observe(1)\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");

    assert!(verify_mir_bubble(&optimized, front_end.types()).is_ok());
    assert!(optimized.dump().contains("do v1 callDirect s0"));
}

#[test]
fn optimization_preserves_narrow_overflow_and_folds_unsigned_ordering() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/numericOptimization.pop",
        "namespace Main\n\
         public function overflowingByte(): UInt8\n\
             return 255 + 1\n\
         end\n\
         public function unusedOverflow()\n\
             local maximum: UInt8 = 255\n\
             maximum + 1\n\
         end\n\
         public function unsignedOrdering(): Boolean\n\
             local high: UInt64 = 18446744073709551615\n\
             local lower: UInt64 = 9223372036854775808\n\
             return high > lower\n\
         end\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");
    let dump = optimized.dump();

    assert_eq!(dump.matches("integer.checkedAdd UInt8").count(), 2);
    assert!(dump.contains("const.integer UInt8 255"));
    assert!(!dump.contains("integer.compareGreater UInt64"));
    assert!(dump.contains("const.boolean true"));
    assert!(verify_mir_bubble(&optimized, front_end.types()).is_ok());
}

#[test]
fn optimization_folds_valid_numeric_conversions_and_preserves_conversion_traps() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/conversionOptimization.pop",
        "namespace Main\n\
         public function valid(): (Float64, Int, Boolean)\n\
             return (Float64(41), Int(2.75), 1.5 <= 2.0)\n\
         end\n\
         public function invalid(): UInt8\n\
             return UInt8(256)\n\
         end\n",
    )
    .expect("source");
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
    let construction =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let optimized = optimize_mir(construction, front_end.types()).expect("optimized MIR");
    let dump = optimized.dump();

    assert!(dump.contains("const.float Float64 0x4044800000000000"));
    assert!(dump.contains("const.integer Int64 2"));
    assert!(dump.contains("const.boolean true"));
    assert!(dump.contains("numeric.integerToInteger Int64 UInt8"));
    assert!(!dump.contains("numeric.integerToFloat"));
    assert!(!dump.contains("numeric.floatToInteger Float64 Int64"));
    assert!(!dump.contains("float.compareLessOrEqual"));
    assert!(verify_mir_bubble(&optimized, front_end.types()).is_ok());
}
