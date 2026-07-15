use pop_backend_api::RuntimeProfile;
use pop_backend_llvm::{LlvmLoweringError, LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{lower_hir_bubble, parse_mir_dump};
use pop_source::SourceFile;
use pop_target::{Endianness, PointerWidth, TargetSpec};
use std::fmt::Write as _;
use std::fs;
use std::num::NonZeroU32;
use std::process::{Command, Output};

fn target() -> TargetSpec {
    TargetSpec::builder("x86_64-unknown-linux-gnu")
        .pointer_width(PointerWidth::Bits64)
        .endianness(Endianness::Little)
        .build()
        .expect("complete target")
}

#[test]
fn lowers_verified_mir_through_private_ir_to_deterministic_llvm_ir() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\nprivate function main(arguments: Array<String>): Int\n    local value: Int = 40 + 2\n    return value\nend\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(front_end.diagnostics().is_empty());
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");

    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default(),
    )
    .expect("LLVM lowering");
    let text = module.to_string();
    assert!(text.contains("target triple = \"x86_64-unknown-linux-gnu\""));
    assert!(text.contains("define i64 @pop_b0_s0(i64 %v0)"));
    assert!(text.contains("add i64"));
    assert!(text.contains("ret i64"));
    assert!(
        text.contains("declare i8 @pop_rt_array_get_checked(i64, i64, ptr) nounwind")
            && text.contains("declare i8 @pop_rt_table_get_checked(i64, i64, i1, ptr) nounwind")
            && text.contains("declare i8 @pop_rt_array_set(i64, i64, i64) nounwind")
            && text.contains("declare i64 @pop_rt_field_get(i64, i64) nounwind")
            && text.contains("declare i8 @pop_rt_field_set(i64, i64, i64) nounwind"),
        "collection and field operations need exact optimizable ABI signatures: {text}"
    );
    assert!(
        !text.contains("@pop_rt_array_get(...)") && !text.contains("@pop_rt_field_set(...)"),
        "variadic runtime declarations hide optimizer-visible argument contracts: {text}"
    );
    assert!(
        !text.contains("pop_rt_semantic"),
        "runtime operations must use closed PLRI identities"
    );
}

#[test]
fn llvm_rejects_async_functions_until_task_state_machine_lowering_exists() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/async.pop",
        "namespace Main\n\
         private async function work(): Int\n\
             return 42\n\
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
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");

    assert!(matches!(
        lower_mir_to_llvm_ir(
            &mir,
            front_end.types(),
            &target(),
            LlvmLoweringOptions::default(),
        ),
        Err(LlvmLoweringError::UnsupportedAsync)
    ));
}

#[test]
fn nominal_enum_constants_and_equality_lower_to_i32() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/enum.pop",
        "namespace Main\n\
         private enum Color\n\
             Red\n\
             Blue\n\
         end\n\
         private function main(arguments: Array<String>): Int\n\
             if Color.Red == Color.Red then\n\
                 return 7\n\
             end\n\
             return 1\n\
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
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect("LLVM lowering");
    let text = module.to_string();
    assert!(text.contains("add i32 0, 0"), "{text}");
    assert!(text.contains("icmp eq i32"), "{text}");
    let result = link_with_runtime_and_run(&module, "enum");
    assert_eq!(result.status.code(), Some(7), "{text}");
}

#[test]
fn optional_presence_and_extraction_use_a_typed_private_llvm_representation() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/optional.pop",
        "namespace Main\n\
         public function choose(value: Int?, fallback: Int): Int\n\
             return value ?? fallback\n\
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
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types())
        .expect("verified optional MIR");
    let text = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default(),
    )
    .expect("LLVM optional lowering")
    .to_string();

    assert!(text.contains("extractvalue { i1, i64 }"), "{text}");
    assert!(!text.to_ascii_lowercase().contains("dynamic"), "{text}");
    let input = std::env::temp_dir().join("pop-backend-llvm-optionals.ll");
    let output = std::env::temp_dir().join("pop-backend-llvm-optionals.bc");
    fs::write(&input, &text).expect("write optional LLVM input");
    let assembled = Command::new("llvm-as")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("llvm-as must be installed");
    assert!(
        assembled.status.success(),
        "llvm-as rejected optional IR: {}\n{text}",
        String::from_utf8_lossy(&assembled.stderr)
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
}

#[test]
fn optional_scalar_collection_reads_execute_without_a_zero_sentinel() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/optionalNative.pop",
        "namespace Main\n\
         function main(): Int\n\
             local values: {Int} = { 0 }\n\
             local present = values[1] ?? 7\n\
             local absent = values[2] ?? 7\n\
             local scores: {[String]: Int} = { zero = 0 }\n\
             local tablePresent = scores[\"zero\"] ?? 7\n\
             local tableAbsent = scores[\"missing\"] ?? 7\n\
             return present * 10 + absent + tablePresent * 10 + tableAbsent\n\
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
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types())
        .expect("verified optional collection MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect("LLVM optional collection lowering");
    let result = link_with_runtime_and_run(&module, "optional-scalar");
    assert_eq!(result.status.code(), Some(14), "{module}");
}

#[test]
fn specialized_generic_data_and_calls_execute_natively() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/generics.pop",
        "namespace Main\n\
         private record Box<T>\n\
             value: T\n\
         end\n\
         private union Choice<T>\n\
             Value(value: T)\n\
             Empty\n\
         end\n\
         private function identity<T>(value: T): T\n\
             return value\n\
         end\n\
         private function boxed<T>(value: T): Box<T>\n\
             local result: Box<T> = { value = identity<<T>>(value) }\n\
             return result\n\
         end\n\
         private function choose<T>(value: T): Choice<T>\n\
             return Choice.Value<<T>>(value)\n\
         end\n\
         private function main(arguments: Array<String>): Int\n\
             local box: Box<Int> = boxed<<Int>>(7)\n\
             local choice: Choice<Int> = choose<<Int>>(box.value)\n\
             match choice\n\
             when Choice.Value(value) then\n\
                 return value\n\
             when Choice.Empty then\n\
                 return 0\n\
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
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let hir = front_end.hir().expect("HIR");
    let entry = hir
        .functions()
        .iter()
        .find(|function| function.name() == "main")
        .expect("entry")
        .symbol();
    let mir = lower_hir_bubble(hir, front_end.types()).expect("specialized MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering");

    let result = link_with_runtime_and_run(&module, "generic-execution");
    assert_eq!(result.status.code(), Some(7), "{module}");
}

#[test]
fn specialized_generic_data_lowers_to_concrete_native_ir() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/generics.pop",
        "namespace Main\n\
         private record Box<T>\n\
             value: T\n\
         end\n\
         private union Choice<T>\n\
             Value(value: T)\n\
             Empty\n\
         end\n\
         private function boxed<T>(value: T): Box<T>\n\
             local result: Box<T> = { value = value }\n\
             return result\n\
         end\n\
         private function choose<T>(value: T): Choice<T>\n\
             return Choice.Value<<T>>(value)\n\
         end\n\
         private function main(arguments: Array<String>): Int\n\
             local box: Box<Int> = boxed<<Int>>(7)\n\
             local choice: Choice<Int> = choose<<Int>>(box.value)\n\
             match choice\n\
             when Choice.Value(value) then\n\
                 return value\n\
             when Choice.Empty then\n\
                 return 0\n\
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
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("concrete MIR");
    let text = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default(),
    )
    .expect("LLVM lowering")
    .to_string();
    assert!(text.contains("pop_rt_allocate_initialized_object"));
    assert!(text.contains("pop_rt_field_set"));
    assert!(text.contains("switch i64"));
    let input = std::env::temp_dir().join("pop-backend-llvm-generics.ll");
    let output = std::env::temp_dir().join("pop-backend-llvm-generics.bc");
    fs::write(&input, text).expect("write generic LLVM input");
    let assembled = Command::new("llvm-as")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("llvm-as must be installed");
    assert!(
        assembled.status.success(),
        "llvm-as rejected specialized generic IR: {}",
        String::from_utf8_lossy(&assembled.stderr)
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
}

#[test]
fn class_construction_uses_one_atomic_initialized_allocation() {
    let module = native_module(
        "namespace Main\n\
class Box\n\
    value: Int\n\
    function Box.new(value: Int): Box\n\
        return Box { value = value }\n\
    end\n\
end\n\
private function main(): Int\n\
    local box = Box.new(42)\n\
    return box.value\n\
end\n",
    );
    let text = module.to_string();
    assert_eq!(
        text.matches("call i64 @pop_rt_allocate_initialized_object")
            .count(),
        1,
        "{text}"
    );
    assert!(!text.contains("call i8 @pop_rt_field_set"), "{text}");

    let result = link_with_runtime_and_run(&module, "initialized-class");
    assert_eq!(result.status.code(), Some(42), "{module}");
}

#[test]
fn class_mutation_after_publication_keeps_the_checked_store_path() {
    let module = native_module(
        "namespace Main\n\
class Box\n\
    value: Int\n\
end\n\
private function main(): Int\n\
    local box = Box { value = 1 }\n\
    box.value = 9\n\
    return box.value\n\
end\n",
    );
    let text = module.to_string();
    assert!(text.contains("call i8 @pop_rt_field_set"), "{text}");
    let result = link_with_runtime_and_run(&module, "mutated-class");
    assert_eq!(result.status.code(), Some(9), "{module}");
}

#[test]
fn fixed_pack_calls_and_multiple_assignment_execute_natively() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/fixedPack.pop",
        "namespace Main\n\
         private function split(value: Int): (Int, Int)\n\
             return value, value + 1\n\
         end\n\
         private function main(arguments: Array<String>): Int\n\
             local left, right = split(10)\n\
             local result = split(10)\n\
             left, right = right, left\n\
             return result[1] + result[2]\n\
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
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[1].symbol()),
    )
    .expect("LLVM lowering");
    let text = module.to_string();
    assert!(
        text.contains("call i64 @pop_rt_allocate_mapped_object"),
        "{text}"
    );
    assert!(text.contains("@pop_rt_field_get"), "{text}");

    let result = link_with_runtime_and_run(&module, "fixed-pack");
    assert_eq!(result.status.code(), Some(21), "{module}");
}

#[test]
fn root_handle_transitions_preserve_the_native_abi_result_and_argument() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 retainRoot v1\n    do v3 releaseRoot v2\n    v4:t{integer} = const.integer Int64 0\n    return (v4)\n",
        integer = integer.raw(),
        array = array.raw(),
    ))
    .expect("root handle MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect("LLVM lowering");
    let text = module.to_string();

    assert!(text.contains("declare i64 @pop_rt_retain_root(i64)"));
    assert!(text.contains("declare i8 @pop_rt_release_root(i64)"));
    assert!(text.contains("%v2 = call i64 @pop_rt_retain_root(i64 %v1)"));
    assert!(text.contains("call i8 @pop_rt_release_root(i64 %v2)"));

    let result = link_with_runtime_and_run(&module, "root-handle");
    assert!(
        result.status.success(),
        "native root-handle program failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn abi_two_safe_points_reload_roots_before_later_uses() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 gcSafePoint sp1 roots (v1)\n    do v3 retainRoot v1\n    do v4 releaseRoot v3\n    v5:t{integer} = const.integer Int64 0\n    return (v5)\n",
        integer = integer.raw(),
        array = array.raw(),
    ))
    .expect("ABI 2 root reload MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default().with_runtime_profile(RuntimeProfile::ProductionGenerational),
    )
    .expect("ABI 2 LLVM lowering");
    module.verify().expect("valid ABI 2 LLVM module");
    let text = module.to_string();

    assert!(
        text.contains("%v2_gc_status = call i8 @pop_rt_gc_safe_point_v2(i32 1"),
        "{text}"
    );
    assert!(
        text.contains("%v2_gc_accepted = icmp eq i8 %v2_gc_status, 1"),
        "ABI 2 must inspect the closed publication status: {text}"
    );
    assert!(
        text.contains("br i1 %v2_gc_accepted, label %v2_poll_continue, label %v2_gc_rejected"),
        "ABI 2 rejection must leave the continuation path: {text}"
    );
    assert!(
        text.contains("v2_gc_rejected:\ncall void @pop_rt_trap()\nunreachable"),
        "ABI 2 rejection must terminate through runtime failure handling: {text}"
    );
    assert!(
        text.contains("%v1_after_v2 = load i64"),
        "root must reload into a new SSA value: {text}"
    );
    assert!(
        text.contains("call i64 @pop_rt_retain_root(i64 %v1_before_v3)"),
        "later managed use must consume the reloaded alias: {text}"
    );
    assert!(
        !text.contains("call i64 @pop_rt_retain_root(i64 %v1)"),
        "old root SSA value survived after the safe point: {text}"
    );
}

#[test]
fn abi_two_root_reload_flows_through_control_flow_arguments() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 gcSafePoint sp1 roots (v1)\n    branch b1 (v1)\n  b1(v3:t{array}):\n    do v4 retainRoot v3\n    do v5 releaseRoot v4\n    v6:t{integer} = const.integer Int64 0\n    return (v6)\n",
        integer = integer.raw(),
        array = array.raw(),
    ))
    .expect("ABI 2 branch reload MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default().with_runtime_profile(RuntimeProfile::ProductionGenerational),
    )
    .expect("ABI 2 branch LLVM lowering");
    module.verify().expect("valid ABI 2 branch LLVM module");
    let text = module.to_string();

    assert!(
        text.contains("%v3 = phi i64 [ %v1_before_b0_exit,"),
        "control-flow merge must receive the reloaded token: {text}"
    );
    assert!(
        !text.contains("%v3 = phi i64 [ %v1,"),
        "control-flow merge retained an old root token: {text}"
    );
    assert!(text.contains("call i64 @pop_rt_retain_root(i64 %v3)"));
}

#[test]
fn abi_two_root_reload_flows_through_loop_backedges() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    branch b1 (v1)\n  b1(v2:t{array}):\n    v3:t{boolean} = const.boolean true\n    condBranch v3 b2 b3\n  b2():\n    do v4 gcSafePoint sp1 roots (v2)\n    branch b1 (v2)\n  b3():\n    do v5 retainRoot v2\n    do v6 releaseRoot v5\n    v7:t{integer} = const.integer Int64 0\n    return (v7)\n",
        integer = integer.raw(),
        boolean = boolean.raw(),
        array = array.raw(),
    ))
    .expect("ABI 2 loop reload MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default().with_runtime_profile(RuntimeProfile::ProductionGenerational),
    )
    .expect("ABI 2 loop LLVM lowering");
    module.verify().expect("valid ABI 2 loop LLVM module");
    let text = module.to_string();

    assert!(
        text.contains("[ %v2_before_b2_exit, %v4_poll_continue ]"),
        "loop backedge must carry the latest relocated token: {text}"
    );
    assert!(
        !text.contains("[ %v2, %v4_poll_continue ]"),
        "loop backedge retained an old root token: {text}"
    );
}

#[test]
fn abi_two_root_reload_survives_a_divergent_control_flow_merge() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    v2:t{boolean} = const.boolean true\n    condBranch v2 b1 b2\n  b1():\n    do v3 gcSafePoint sp1 roots (v1)\n    branch b3 ()\n  b2():\n    branch b3 ()\n  b3():\n    do v4 retainRoot v1\n    do v5 releaseRoot v4\n    v6:t{integer} = const.integer Int64 0\n    return (v6)\n",
        integer = integer.raw(),
        boolean = boolean.raw(),
        array = array.raw(),
    ))
    .expect("ABI 2 divergent merge MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default()
            .with_entry_point(mir.functions()[0].symbol())
            .with_runtime_profile(RuntimeProfile::ProductionGenerational)
            .with_gc_poll_interval(NonZeroU32::MIN),
    )
    .expect("ABI 2 divergent merge LLVM lowering");
    module
        .verify()
        .expect("valid ABI 2 divergent merge LLVM module");
    let text = module.to_string();

    assert!(
        !text.contains("call i64 @pop_rt_retain_root(i64 %v1)"),
        "a join reached from a relocating path must not use the old token: {text}"
    );
    let result = link_with_forced_relocation_runtime_and_run(&text, "abi-two-divergent-merge");
    assert!(
        result.status.success(),
        "optimized divergent merge lost a relocated token: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stale = text.replacen(
        "call i64 @pop_rt_retain_root(i64 %v1_before_v4)",
        "call i64 @pop_rt_retain_root(i64 %v1)",
        1,
    );
    assert_ne!(stale, text, "merge mutation must restore the old token");
    assert!(
        !link_with_forced_relocation_runtime_and_run(&stale, "abi-two-stale-merge")
            .status
            .success(),
        "the forced-relocation runtime accepted a stale merge token"
    );
}

#[test]
fn optimized_abi_two_execution_carries_relocated_tokens_over_loop_backedges() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    v2:t{boolean} = const.boolean true\n    branch b1 (v1, v2)\n  b1(v3:t{array}, v4:t{boolean}):\n    condBranch v4 b2 b3\n  b2():\n    v5:t{boolean} = const.boolean false\n    do v6 gcSafePoint sp1 roots (v3)\n    branch b1 (v3, v5)\n  b3():\n    do v7 retainRoot v3\n    do v8 releaseRoot v7\n    v9:t{integer} = const.integer Int64 0\n    return (v9)\n",
        integer = integer.raw(),
        boolean = boolean.raw(),
        array = array.raw(),
    ))
    .expect("forced loop relocation MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default()
            .with_entry_point(mir.functions()[0].symbol())
            .with_runtime_profile(RuntimeProfile::ProductionGenerational)
            .with_gc_poll_interval(NonZeroU32::MIN),
    )
    .expect("forced loop relocation LLVM lowering");
    module
        .verify()
        .expect("valid forced loop relocation LLVM module");
    let text = module.to_string();

    let result = link_with_forced_relocation_runtime_and_run(&text, "abi-two-loop-backedge");
    assert!(
        result.status.success(),
        "optimized loop backedge lost a relocated token: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stale = text.replacen(
        "[ %v3_before_b2_exit, %v6_poll_continue ]",
        "[ %v3, %v6_poll_continue ]",
        1,
    );
    assert_ne!(stale, text, "loop mutation must restore the old token");
    assert!(
        !link_with_forced_relocation_runtime_and_run(&stale, "abi-two-stale-loop")
            .status
            .success(),
        "the forced-relocation runtime accepted a stale loop token"
    );
}

#[test]
fn abi_two_repeated_safe_points_chain_reloaded_roots() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 gcSafePoint sp1 roots (v1)\n    do v3 gcSafePoint sp2 roots (v1)\n    do v4 retainRoot v1\n    do v5 releaseRoot v4\n    v6:t{integer} = const.integer Int64 0\n    return (v6)\n",
        integer = integer.raw(),
        array = array.raw(),
    ))
    .expect("ABI 2 repeated safe-point MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default().with_runtime_profile(RuntimeProfile::ProductionGenerational),
    )
    .expect("ABI 2 repeated safe-point LLVM lowering");
    module
        .verify()
        .expect("valid ABI 2 repeated safe-point LLVM module");
    let text = module.to_string();

    assert!(
        text.contains("store i64 %v1_before_v3, ptr %v3_roots_0"),
        "the second publication must spill the first reload: {text}"
    );
    assert!(
        text.contains("%v1_after_v3 = load i64"),
        "the second safe point must define a fresh reload: {text}"
    );
    assert!(
        text.contains("call i64 @pop_rt_retain_root(i64 %v1_before_v4)"),
        "later uses must consume the newest reload: {text}"
    );
}

#[test]
fn optimized_abi_two_execution_rejects_stale_tokens_after_forced_relocation() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 gcSafePoint sp1 roots (v1)\n    do v3 retainRoot v1\n    do v4 releaseRoot v3\n    v5:t{integer} = const.integer Int64 0\n    return (v5)\n",
        integer = integer.raw(),
        array = array.raw(),
    ))
    .expect("forced relocation MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default()
            .with_entry_point(mir.functions()[0].symbol())
            .with_runtime_profile(RuntimeProfile::ProductionGenerational)
            .with_gc_poll_interval(NonZeroU32::MIN),
    )
    .expect("forced relocation LLVM lowering");
    module
        .verify()
        .expect("valid forced relocation LLVM module");

    let text = module.to_string();
    assert!(
        text.contains("declare i8 @pop_rt_supports_abi(i16, i16)"),
        "ABI 2 entry must declare exact descriptor negotiation: {text}"
    );
    assert!(
        text.contains("call i8 @pop_rt_supports_abi(i16 2, i16 0)"),
        "ABI 2 entry must validate the complete linked descriptor: {text}"
    );
    let result = link_with_forced_relocation_runtime_and_run(&text, "abi-two-relocation");
    assert!(
        result.status.success(),
        "optimized native execution used a stale token: {}\n{module}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stable_result = link_with_runtime_and_run(&module, "abi-two-stable-rejection");
    assert!(
        !stable_result.status.success(),
        "the stable ABI 1 facade must reject an ABI 2 executable before normal entry"
    );

    let stale = text.replacen(
        "call i64 @pop_rt_retain_root(i64 %v1_before_v3)",
        "call i64 @pop_rt_retain_root(i64 %v1)",
        1,
    );
    assert_ne!(stale, text, "test mutation must restore the old SSA token");
    let stale_result = link_with_forced_relocation_runtime_and_run(&stale, "abi-two-stale-token");
    assert!(
        !stale_result.status.success(),
        "the forced-relocation runtime accepted an old token"
    );
}

#[test]
fn pin_transitions_preserve_the_native_abi_result_and_argument() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let mir = parse_mir_dump(&format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> (t{integer}) effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 pin v1\n    do v3 unpin v2\n    v4:t{integer} = const.integer Int64 0\n    return (v4)\n",
        integer = integer.raw(),
        array = array.raw(),
    ))
    .expect("pin MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        &types,
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect("LLVM lowering");
    let text = module.to_string();

    assert!(text.contains("declare i64 @pop_rt_pin(i64)"));
    assert!(text.contains("declare i8 @pop_rt_unpin(i64)"));
    assert!(text.contains("%v2 = call i64 @pop_rt_pin(i64 %v1)"));
    assert!(text.contains("call i8 @pop_rt_unpin(i64 %v2)"));

    let result = link_with_runtime_and_run(&module, "pin-handle");
    assert!(
        result.status.success(),
        "native pin-handle program failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_text_is_accepted_by_llvm_as() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\nprivate function main(arguments: Array<String>): Int\n    return 40 + 2\nend\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let text = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect("LLVM lowering")
    .to_string();
    let input = std::env::temp_dir().join("pop-backend-llvm-conformance.ll");
    let output = std::env::temp_dir().join("pop-backend-llvm-conformance.bc");
    fs::write(&input, text).expect("write temporary LLVM input");
    let result = Command::new("llvm-as")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("llvm-as must be installed for the native backend conformance test");
    assert!(
        result.status.success(),
        "llvm-as rejected generated IR: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
}

#[test]
fn emitted_llvm_executes_a_pure_pop_function() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\nprivate function main(arguments: Array<String>): Int\n    return 42\nend\n",
    )
    .expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect("LLVM lowering");
    let text = module.to_string();
    assert!(text.contains("define i32 @main(i32 %pop_argc, ptr %pop_argv)"));
    assert!(text.contains("call i64 @pop_rt_process_arguments"));
    assert!(text.contains("call i64 @pop_b0_s0(i64 %pop_arguments)"));
    assert!(
        !text.contains("pop_rt_supports_abi"),
        "ABI 1 entry must retain its fixed bootstrap descriptor"
    );
    let result = link_with_runtime_and_run(&module, "pure-entry");
    assert_eq!(
        result.status.code(),
        Some(42),
        "lli rejected or failed generated IR: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn no_argument_no_result_entry_returns_zero_without_decoding_arguments() {
    let module = native_module(
        "namespace Main\n\
private function main()\n\
end\n",
    );
    let text = module.to_string();
    assert!(text.contains("call void @pop_b0_s0()"));
    assert!(text.contains("ret i32 0"));
    assert!(!text.contains("call i64 @pop_rt_process_arguments"));
    let result = link_with_runtime_and_run(&module, "clean-entry");
    assert!(
        result.status.success(),
        "clean entry failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_nested_control_flow_and_typed_helper_returns() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
private function choose(left: Int, right: Int): Int\n\
    if left < right then\n\
        return left + right\n\
    else\n\
        return right\n\
    end\n\
end\n\
private function enabled(): Boolean\n\
    return true\n\
end\n\
private function count(): Int\n\
    local value = 0\n\
    while value < 42 do\n\
        value = value + 1\n\
    end\n\
    return value\n\
end\n\
private function idle()\n\
    while false do\n\
    end\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    idle()\n\
    if enabled() then\n\
        return choose(0, count())\n\
    else\n\
        return 1\n\
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
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let entry = mir
        .functions()
        .iter()
        .find(|function| function.symbol().raw() == 4)
        .expect("run")
        .symbol();
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering");
    let result = link_with_runtime_and_run(&module, "control-flow");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted control flow: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_luau_shaped_repeat_until_control_flow() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local value = 0\n\
    repeat\n\
        value = value + 1\n\
    until value == 42\n\
    return value\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "repeat-until");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted repeat-until control flow: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_numeric_ranges_break_and_continue() {
    let module = native_module(
        "namespace Main\n\
private function main(arguments: Array<String>): Int\n\
    local total = 0\n\
    for index = 1, 6 do\n\
        if index == 2 then\n\
            continue\n\
        end\n\
        if index == 5 then\n\
            break\n\
        end\n\
        total = total + index\n\
    end\n\
    for reverse = 3, 1, -1 do\n\
        total = total + reverse\n\
    end\n\
    return total\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "numeric-for-range");
    assert_eq!(
        result.status.code(),
        Some(14),
        "native executable misexecuted numeric ranges: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_static_generalized_iteration() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local values: {Int} = { 10, 20, 12 }\n\
    local total = 0\n\
    for value in values do\n\
        total = total + value\n\
    end\n\
    return total\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "generalized-iteration");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted generalized iteration: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_first_class_integer_ranges() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local total = 0\n\
    for value in Range.create(1, 5, 2) do\n\
        total += value\n\
    end\n\
    for value in Range.create(5, 1, -2) do\n\
        total += value\n\
    end\n\
    for value in Range.create(5, 1) do\n\
        total += 100\n\
    end\n\
    return total\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "first-class-ranges");
    assert_eq!(
        result.status.code(),
        Some(18),
        "native executable misexecuted first-class ranges: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_does_not_advance_a_broken_range() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local first = Int8(126)\n\
    local last = Int8(127)\n\
    local step = Int8(2)\n\
    for value in Range.create(first, last, step) do\n\
        return Int(value)\n\
    end\n\
    return 0\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "range-break-before-overflow");
    assert_eq!(
        result.status.code(),
        Some(126),
        "native executable advanced a broken range: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_keeps_iterator_cleanup_explicit() {
    let module = native_module(
        "namespace Main\n\
private class ResourceIterator implements Iterator<Int>\n\
    private current: Int\n\
    private closed: Boolean\n\
    public function ResourceIterator.new(): ResourceIterator\n\
        return ResourceIterator { current = 1, closed = false }\n\
    end\n\
    public function ResourceIterator:iterator(): Iterator<Int>\n\
        return self\n\
    end\n\
    public function ResourceIterator:next(): Iteration<Int>\n\
        if self.current > 1 then\n\
            return Iteration.End\n\
        end\n\
        self.current += 1\n\
        return Iteration.Item(1)\n\
    end\n\
    public function ResourceIterator:close()\n\
        self.closed = true\n\
    end\n\
    public function ResourceIterator:isClosed(): Boolean\n\
        return self.closed\n\
    end\n\
end\n\
private function consumeWithCleanup(iterator: ResourceIterator): Boolean\n\
    defer\n\
        iterator:close()\n\
    end\n\
    for value in iterator do\n\
        break\n\
    end\n\
    return iterator:isClosed()\n\
end\n\
private function main(): Int\n\
    local withoutCleanup = ResourceIterator.new()\n\
    for value in withoutCleanup do\n\
        break\n\
    end\n\
    local withCleanup = ResourceIterator.new()\n\
    local closedBeforeReturn = consumeWithCleanup(withCleanup)\n\
    if not withoutCleanup:isClosed() and not closedBeforeReturn and withCleanup:isClosed() then\n\
        return 42\n\
    end\n\
    return 1\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "explicit-iterator-cleanup");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable changed explicit iterator cleanup: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_growable_list_core_and_iteration() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local values = List.withCapacity<<Int>>(1)\n\
    List.add(values, 0)\n\
    List.add(values, 40)\n\
    values[1] = 2\n\
    local total = 0\n\
    for value in values do\n\
        total += value\n\
    end\n\
    return total + List.length(values) - List.get(values, 2)\n\
end\n",
    );
    let text = module.to_string();
    assert!(text.contains("call i64 @pop_rt_list_create"), "{text}");
    assert!(text.contains("call i8 @pop_rt_list_add"), "{text}");
    assert!(text.contains("call i8 @pop_rt_list_set"), "{text}");
    let result = link_with_runtime_and_run(&module, "growable-list");
    assert_eq!(
        result.status.code(),
        Some(4),
        "native executable misexecuted growable List: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_lazy_ordinary_pop_sequence_adapters() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local values: {Int} = {1, 2, 3}\n\
                 local mapped = map(values, function(value: Int): Int\n\
                     return value * 2\n\
                 end)\n\
                 local filtered = filter(mapped, function(value: Int): Boolean\n\
                     return value > 2\n\
                 end)\n\
                 local collected = collect(filtered)\n\
                 return List.get(collected, 1) + List.get(collected, 2)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "ordinary-pop-sequence");
    assert_eq!(
        result.status.code(),
        Some(10),
        "native executable misexecuted Sequence adapters: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_short_circuiting_sequence_aggregates() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local values: {Int} = {1, 2, 3, 4}\n\
                 local anyCalls = 0\n\
                 local found = any(values, function(value: Int): Boolean\n\
                     anyCalls += 1\n\
                     return value > 2\n\
                 end)\n\
                 local allCalls = 0\n\
                 local matched = all(values, function(value: Int): Boolean\n\
                     allCalls += 1\n\
                     return value < 3\n\
                 end)\n\
                 if not found or matched then\n\
                     return -1\n\
                 end\n\
                 return anyCalls * 10 + allCalls + count(values)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "sequence-aggregates");
    assert_eq!(
        result.status.code(),
        Some(37),
        "native executable misexecuted Sequence aggregates: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_sequence_inspection_and_visitation() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local empty: {Int} = {}\n\
                 local single: {Int} = {9}\n\
                 local values: {Int} = {1, 2, 3, 4}\n\
                 if not isEmpty(empty) or isEmpty(values) then\n\
                     return -1\n\
                 end\n\
                 local total = 0\n\
                 each(values, function(value: Int)\n\
                     total += value\n\
                 end)\n\
                 local matches = countWhere(values, function(value: Int): Boolean\n\
                     return value % 2 == 0\n\
                 end)\n\
                 if not none(values, function(value: Int): Boolean\n\
                     return value > 4\n\
                 end) then\n\
                     return -1\n\
                 end\n\
                 local noneCalls = 0\n\
                 local noEven = none(values, function(value: Int): Boolean\n\
                     noneCalls += 1\n\
                     return value == 2\n\
                 end)\n\
                 if noEven or noneCalls ~= 2 then\n\
                     return -1\n\
                 end\n\
                 return firstOr(values, 20) + lastOr(values, 20) * 2 + firstOr(empty, 7) + lastOr(empty, 8) + firstOr(single, 0) + lastOr(single, 0) + total + matches\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "sequence-inspection-visitation");
    assert_eq!(
        result.status.code(),
        Some(54),
        "native executable misexecuted Sequence terminals: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_integer_sequence_aggregates() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local empty: {Int} = {}\n\
                 local values: {Int} = {2, 3, 4}\n\
                 return sum(values) + product(values) + minOr(values, 100) + maxOr(values, -100) + sum(empty) + product(empty) + minOr(empty, 7) + maxOr(empty, 8)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "integer-sequence-aggregates");
    assert_eq!(
        result.status.code(),
        Some(55),
        "native executable misexecuted integer Sequence aggregates: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_sequence_projection_and_composition() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local values: {Int} = {3, 1, 2}\n\
                 local found = findOr(values, function(value: Int): Boolean\n\
                     return value == 2\n\
                 end, 9)\n\
                 local position = indexOr(values, function(value: Int): Boolean\n\
                     return value == 2\n\
                 end, -1)\n\
                 local total = sumBy(values, function(value: Int): Int\n\
                     return value * 2\n\
                 end)\n\
                 local appended = collect(append(values, 9))\n\
                 local prepended = collect(prepend(values, 8))\n\
                 local states = scan(values, 10, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end)\n\
                 return found + position + total + List.get(appended, 4) + List.get(prepended, 1) + sum(states)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "sequence-projection-composition");
    assert_eq!(
        result.status.code(),
        Some(77),
        "native executable misexecuted projected Sequence operations: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_preserves_projection_counts_ties_and_generic_items() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\nusing Pop.Sequence\nprivate record Candidate\n    id: Int\n    key: Int\nend\nprivate function main(): Int\n    local first: Candidate = { id = 1, key = 5 }\n    local second: Candidate = { id = 2, key = 5 }\n    local third: Candidate = { id = 3, key = 7 }\n    local fourth: Candidate = { id = 4, key = 7 }\n    local candidates: {Candidate} = {first, second, third, fourth}\n    local minCalls = 0\n    local least = minByOr(candidates, function(value: Candidate): Int\n        minCalls += 1\n        return value.key\n    end, third)\n    local maxCalls = 0\n    local greatest = maxByOr(candidates, function(value: Candidate): Int\n        maxCalls += 1\n        return value.key\n    end, first)\n    local words: {String} = {\"first\", \"match\", \"last\"}\n    local word = findOr(words, function(value: String): Boolean\n        return value == \"match\"\n    end, \"missing\")\n    if least.id ~= 1 or greatest.id ~= 3 then\n        return 1\n    end\n    if minCalls ~= 4 or maxCalls ~= 4 then\n        return 2\n    end\n    if word ~= \"match\" then\n        return 3\n    end\n    return 0\nend\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "sequence-projection-contract");
    assert_eq!(
        result.status.code(),
        Some(0),
        "native projection contract failed: {}\n{module}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn emitted_llvm_preserves_lazy_adapter_exhaustion() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        ("src/main.pop", include_str!("sequenceLazyExhaustion.pop")),
    ]);
    let result = link_with_runtime_and_run(&module, "sequence-lazy-exhaustion");
    assert_eq!(
        result.status.code(),
        Some(0),
        "native lazy adapter contract failed: {}\n{module}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn emitted_llvm_preserves_integer_sequence_sum_overflow() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local values: {Int} = {9223372036854775807, 1}\n\
                 return sum(values)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "integer-sequence-sum-overflow");
    assert!(
        result.status.code().is_none(),
        "Sequence.sum must preserve checked Int overflow\n{module}"
    );
}

#[test]
fn emitted_llvm_preserves_integer_sequence_product_overflow() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local values: {Int} = {9223372036854775807, 2}\n\
                 return product(values)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "integer-sequence-product-overflow");
    assert!(
        result.status.code().is_none(),
        "Sequence.product must preserve checked Int overflow\n{module}"
    );
}

#[test]
fn emitted_llvm_preserves_projected_sequence_sum_overflow() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\nusing Pop.Sequence\nprivate function main(): Int\n    local values: {Int} = {9223372036854775807, 1}\n    return sumBy(values, function(value: Int): Int\n        return value\n    end)\nend\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "projected-sequence-sum-overflow");
    assert!(
        result.status.code().is_none(),
        "Sequence.sumBy must preserve checked Int overflow\n{module}"
    );
}

#[test]
fn emitted_llvm_preserves_projected_sequence_product_overflow() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\nusing Pop.Sequence\nprivate function main(): Int\n    local values: {Int} = {9223372036854775807, 2}\n    return productBy(values, function(value: Int): Int\n        return value\n    end)\nend\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "projected-sequence-product-overflow");
    assert!(
        result.status.code().is_none(),
        "Sequence.productBy must preserve checked Int overflow\n{module}"
    );
}

#[test]
fn emitted_llvm_executes_exact_source_overloads() {
    let module = native_modules(&[
        (
            "src/int.pop",
            "namespace Main\npublic function choose(value: Int): Int return value + 1 end\n",
        ),
        (
            "src/text.pop",
            "namespace Main\npublic function choose(value: String): String return value .. \"!\" end\n",
        ),
        (
            "src/main.pop",
            "namespace Main\nprivate function main(): Int\n    if choose(\"pop\") ~= \"pop!\" then\n        return 1\n    end\n    if choose(41) ~= 42 then\n        return 2\n    end\n    return 0\nend\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "exact-source-overloads");
    assert_eq!(
        result.status.code(),
        Some(0),
        "native overload execution failed: {}\n{module}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn emitted_llvm_executes_sequence_index_last_and_reduction() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            include_str!(
                "../../../../libraries/standard/tests/programs/sequenceIndexLastReduction.pop"
            ),
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "sequence-index-last-reduction");
    assert_eq!(
        result.status.code(),
        Some(0),
        "native Sequence index/last/reduction contract failed: {}\n{module}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn emitted_llvm_executes_lazy_sequence_bounds_and_composition() {
    let module = native_modules(&[
        (
            "src/sequence.pop",
            include_str!("../../../../libraries/standard/pop/src/sequence.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Sequence\n\
             private function main(): Int\n\
                 local empty: {Int} = {}\n\
                 local single: {Int} = {9}\n\
                 local values: {Int} = {1, 2, 3, 4, 5}\n\
                 if count(take(values, -1)) ~= 0 or count(take(values, 0)) ~= 0 or count(take(values, 10)) ~= 5 then\n\
                     return -1\n\
                 end\n\
                 if count(drop(values, -1)) ~= 5 or count(drop(values, 10)) ~= 0 then\n\
                     return -1\n\
                 end\n\
                 local takeCalls = 0\n\
                 local prefix = takeWhile(values, function(value: Int): Boolean\n\
                     takeCalls += 1\n\
                     return value < 4\n\
                 end)\n\
                 local prefixSum = fold(prefix, 0, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end)\n\
                 local dropCalls = 0\n\
                 local suffix = dropWhile(values, function(value: Int): Boolean\n\
                     dropCalls += 1\n\
                     return value < 3\n\
                 end)\n\
                 local suffixSum = fold(suffix, 0, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end)\n\
                 if takeCalls ~= 4 or dropCalls ~= 3 or count(prefix) ~= 0 then\n\
                     return -1\n\
                 end\n\
                 local takeSum = fold(take(values, 3), 0, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end)\n\
                 local dropSum = fold(drop(values, 2), 0, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end)\n\
                 local joinedSum = fold(concat(take(values, 2), drop(values, 3)), 0, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end)\n\
                 local edgeSum = fold(concat(empty, single), 0, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end) + fold(concat(single, empty), 0, function(state: Int, value: Int): Int\n\
                     return state + value\n\
                 end)\n\
                 return takeSum + dropSum + prefixSum + suffixSum + joinedSum + edgeSum + takeCalls + dropCalls\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "sequence-bounds-composition");
    assert_eq!(
        result.status.code(),
        Some(73),
        "native executable misexecuted lazy Sequence adapters: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_portable_integer_math() {
    let module = native_modules(&[
        (
            "src/math.pop",
            include_str!("../../../../libraries/standard/pop/src/math.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Math\n\
             private function main(): Int\n\
                 local values = min(7, 3) + max(-2, 5) + abs(-4) + gcd(54, -24)\n\
                 local numberTheory = lcm(21, -6) + sign(-20)\n\
                 if not coprime(35, 64) or coprime(21, 6) or lcm(3000000000, 6000000000) ~= 6000000000 or lcm(-9223372036854775807 - 1, 0) ~= 0 then\n\
                     return -1\n\
                 end\n\
                 return values + numberTheory\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "portable-integer-math");
    assert_eq!(
        result.status.code(),
        Some(59),
        "native executable misexecuted portable Math: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_preserves_portable_math_overflow() {
    let module = native_modules(&[
        (
            "src/math.pop",
            include_str!("../../../../libraries/standard/pop/src/math.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Math\n\
             private function main(): Int\n\
                 local minimum = -9223372036854775807 - 1\n\
                 return abs(minimum)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "portable-math-overflow");
    assert!(
        result.status.code().is_none(),
        "Math.abs must preserve checked Int overflow\n{module}"
    );
}

#[test]
fn emitted_llvm_preserves_portable_lcm_overflow() {
    let module = native_modules(&[
        (
            "src/math.pop",
            include_str!("../../../../libraries/standard/pop/src/math.pop"),
        ),
        (
            "src/main.pop",
            "namespace Main\n\
             using Pop.Math\n\
             private function main(): Int\n\
                 return lcm(9223372036854775807, 2)\n\
             end\n",
        ),
    ]);
    let result = link_with_runtime_and_run(&module, "portable-lcm-overflow");
    assert!(
        result.status.code().is_none(),
        "Math.lcm must preserve checked Int overflow\n{module}"
    );
}

#[test]
fn loop_safe_points_lower_to_an_llvm_promotable_function_local_poll() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local value = 0\n\
    repeat\n\
        value = value + 1\n\
    until value == 42\n\
    return value\n\
end\n",
    );
    let text = module.to_string();

    assert!(
        text.contains("%pop_gc_poll_budget = alloca i32")
            && text.contains("store i32 16384, ptr %pop_gc_poll_budget"),
        "LLVM needs a function-local poll budget that mem2reg can promote:\n{text}"
    );
    assert!(
        text.contains("load i32, ptr %pop_gc_poll_budget"),
        "the loop backedge must use the cheap poll path:\n{text}"
    );
    assert!(
        !text.contains("thread_local"),
        "the hot loop must not perform a TLS load and store on every backedge:\n{text}"
    );
    assert!(
        text.contains("_poll_slow:")
            && text.contains("call i8 @pop_rt_gc_safe_point(i32 0, ptr null, i64 0)"),
        "an expired budget must retain the precise runtime safe point:\n{text}"
    );
    assert!(
        text.contains("call i1 @llvm.expect.i1")
            && text.contains("declare i8 @pop_rt_gc_safe_point(i32, ptr, i64) cold nounwind"),
        "LLVM must see the runtime poll as an unlikely cold path:\n{text}"
    );
}

#[test]
fn non_escaping_scalar_arrays_lower_to_direct_llvm_storage() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local values = Array.create<<Int>>(4, 0)\n\
    Array.fill(values, 7)\n\
    values[1] = 3\n\
    return Array.length(values) + Array.get(values, 1)\n\
end\n",
    );
    let text = module.to_string();
    let function = text
        .split("define internal i64 @pop_b0_s0()")
        .nth(1)
        .and_then(|text| text.split("\n}\n").next())
        .expect("lowered scalar array function");
    assert!(function.contains("call noalias ptr @malloc"));
    assert!(function.contains("getelementptr i64"));
    assert!(function.contains("load i64"));
    assert!(function.contains("store i64"));
    assert!(function.contains("call void @free"));
    assert!(!function.contains("pop_rt_allocate_array_filled"));
    assert!(!function.contains("pop_rt_array_length"));
    assert!(!function.contains("pop_rt_array_get_checked"));
    assert!(!function.contains("pop_rt_array_fill"));
    assert!(!function.contains("pop_rt_array_set"));

    let result = link_with_runtime_and_run(&module, "fixed-array-core");
    assert_eq!(
        result.status.code(),
        Some(7),
        "native fixed arrays failed: {}\n{text}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn escaping_and_managed_arrays_retain_the_runtime_path() {
    let module = native_module(
        "namespace Main\n\
private function makeValues(): Array<Int>\n\
    return Array.create<<Int>>(2, 20)\n\
end\n\
private function main(): Int\n\
    local values = makeValues()\n\
    local names = Array.create<<String>>(1, \"Pop\")\n\
    names[1] = \"Lang\"\n\
    return Array.get(values, 1) + Array.length(names) + 1\n\
end\n",
    );
    let text = module.to_string();
    assert!(text.contains("call i64 @pop_rt_allocate_array_filled"));
    assert!(text.contains("call i8 @pop_rt_array_get_checked"));
    assert!(text.contains("call i8 @pop_rt_array_set"));

    let result = link_with_runtime_and_run(&module, "runtime-array-boundary");
    assert_eq!(
        result.status.code(),
        Some(22),
        "runtime array boundary failed: {}\n{text}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn loop_carried_scalar_arrays_keep_direct_access_and_precise_gc_roots() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local values = Array.create<<Int>>(10, 0)\n\
    local index = 1\n\
    repeat\n\
        values[index] = index\n\
        index = index + 1\n\
    until index == 11\n\
    index = 1\n\
    local total = 0\n\
    repeat\n\
        total = total + Array.get(values, index)\n\
        index = index + 1\n\
    until index == 11\n\
    return total\n\
end\n",
    );
    let text = module.to_string();
    let function = text
        .split("define internal i64 @pop_b0_s0()")
        .nth(1)
        .and_then(|text| text.split("\n}\n").next())
        .expect("lowered scalar array loop");
    assert!(!function.contains("pop_rt_allocate_array_filled"));
    assert!(!function.contains("pop_rt_array_get_checked"));
    assert!(!function.contains("pop_rt_array_set"));
    assert!(function.contains("getelementptr i64"));
    assert!(function.contains("call i8 @pop_rt_gc_safe_point"));
    assert!(
        function.contains("ptr null, i64 0"),
        "backend-private scalar storage must not enter precise managed roots: {function}"
    );

    let result = link_with_runtime_and_run(&module, "direct-scalar-array-loop");
    assert_eq!(
        result.status.code(),
        Some(55),
        "direct scalar array loop failed: {}\n{text}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn read_only_loop_local_scalar_arrays_are_replaced_without_allocation() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local index = 1\n\
    local total = 0\n\
    repeat\n\
        local values = Array.create<<Int>>(256, index)\n\
        total = total + Array.get(values, 1)\n\
        index = index + 1\n\
    until index == 201\n\
    return total\n\
end\n",
    );
    let text = module.to_string();
    let function = text
        .split("define internal i64 @pop_b0_s0()")
        .nth(1)
        .and_then(|text| text.split("\n}\n").next())
        .expect("lowered allocation-churn loop");

    assert!(
        !function.contains("pop_rt_allocate_array_filled"),
        "{function}"
    );
    assert!(!function.contains("pop_rt_array_get_checked"), "{function}");
    assert!(!function.contains("call noalias ptr @malloc"), "{function}");
    assert!(
        function.contains("_length_nonnegative = icmp sge i64"),
        "{function}"
    );
    assert!(function.contains("_in_bounds = icmp ult i64"), "{function}");
    assert!(function.contains("call void @pop_rt_trap()"), "{function}");
    assert!(
        function.contains("call i8 @pop_rt_gc_safe_point"),
        "{function}"
    );

    let result = link_with_runtime_and_run(&module, "scalar-replaced-churn-loop");
    assert_eq!(
        result.status.code(),
        Some(132),
        "scalar-replaced churn loop failed: {}\n{function}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn mutated_loop_local_scalar_arrays_retain_the_managed_runtime_path() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local index = 1\n\
    local total = 0\n\
    repeat\n\
        local values = Array.create<<Int>>(2, index)\n\
        values[1] = index + 1\n\
        total = total + Array.get(values, 1)\n\
        index = index + 1\n\
    until index == 3\n\
    return total\n\
end\n",
    );
    let text = module.to_string();
    let function = text
        .split("define internal i64 @pop_b0_s0()")
        .nth(1)
        .and_then(|text| text.split("\n}\n").next())
        .expect("lowered mutated array loop");

    assert!(
        function.contains("pop_rt_allocate_array_filled"),
        "{function}"
    );
    assert!(function.contains("pop_rt_array_set"), "{function}");
    assert!(function.contains("pop_rt_array_get_checked"), "{function}");
}

#[test]
fn constant_bounded_integer_reductions_elide_proven_overflow_edges() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local index = 1\n\
    local total = 0\n\
    repeat\n\
        total = total + index\n\
        index = index + 1\n\
    until index == 50000001\n\
    return total\n\
end\n",
    );
    let text = module.to_string();
    let function = text
        .split("define internal i64 @pop_b0_s0()")
        .nth(1)
        .and_then(|text| text.split("\n}\n").next())
        .expect("lowered counted reduction");

    assert_eq!(function.matches("add nsw i64").count(), 2, "{function}");
    assert!(
        !function.contains("with.overflow") && !function.contains("_overflow_expected"),
        "range-proven adds must not retain impossible trap edges: {function}"
    );
    assert!(
        function.contains("pop_rt_gc_safe_point"),
        "range optimization must preserve the mandatory loop poll: {function}"
    );
}

#[test]
fn potentially_overflowing_integer_reductions_keep_checked_edges() {
    let module = native_module(
        "namespace Main\n\
private function main(): Int\n\
    local index = 1\n\
    local total = 9223372036854775800\n\
    repeat\n\
        total = total + index\n\
        index = index + 1\n\
    until index == 10\n\
    return total\n\
end\n",
    );
    let text = module.to_string();
    let function = text
        .split("define internal i64 @pop_b0_s0()")
        .nth(1)
        .and_then(|text| text.split("\n}\n").next())
        .expect("lowered potentially overflowing reduction");

    assert!(
        function.contains("llvm.sadd.with.overflow.i64") && function.contains("_overflow_expected"),
        "an unproven reduction must retain its checked overflow path: {function}"
    );
}

#[test]
fn executable_functions_use_internal_linkage_and_effect_derived_attributes() {
    let module = native_module(
        "namespace Main\n\
private function fibonacci(value: Int): Int\n\
    if value < 2 then\n\
        return value\n\
    end\n\
    return fibonacci(value - 1) + fibonacci(value - 2)\n\
end\n\
private function main(): Int\n\
    return fibonacci(10)\n\
end\n",
    );
    let text = module.to_string();

    assert!(
        text.contains("define internal i64 @pop_b0_s0(i64 %v0) memory(none) nounwind"),
        "a whole-program pure helper must expose optimization-safe attributes:\n{text}"
    );
    assert!(
        text.contains("define internal i64 @pop_b0_s1() memory(none) nounwind"),
        "the Pop entry implementation is module-private behind C main:\n{text}"
    );
    assert!(
        text.contains("declare void @pop_rt_trap() cold noreturn nounwind"),
        "checked arithmetic failure must be outlined as a cold terminal edge:\n{text}"
    );
}

#[test]
fn object_emission_runs_the_llvm_optimization_pipeline() {
    let module = native_module(
        "namespace Main\n\
private function increment(value: Int): Int\n\
    return value + 1\n\
end\n\
private function main(): Int\n\
    return increment(41)\n\
end\n",
    );
    let object = std::env::temp_dir().join(format!(
        "pop-backend-llvm-optimized-object-{}.o",
        std::process::id()
    ));
    module
        .emit_object(&object)
        .expect("optimized object emission");
    let disassembly = Command::new("objdump")
        .args(["-dr", "--no-show-raw-insn"])
        .arg(&object)
        .output()
        .expect("objdump must be installed for LLVM object conformance");
    let _ = fs::remove_file(object);
    assert!(
        disassembly.status.success(),
        "objdump failed: {}",
        String::from_utf8_lossy(&disassembly.stderr)
    );
    let disassembly = String::from_utf8(disassembly.stdout).expect("UTF-8 disassembly");
    let main = disassembly
        .split("<main>:\n")
        .nth(1)
        .and_then(|text| text.split("\n\n").next())
        .expect("main disassembly");
    assert!(
        !main.contains("pop_b0_s0"),
        "LLVM did not inline and fold the private helper:\n{main}"
    );
}

#[test]
fn emitted_llvm_executes_exhaustive_union_switches_with_typed_payloads() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
public union ResultValue\n\
    Ok(value: Int)\n\
    Error(message: String)\n\
end\n\
private function consume(result: ResultValue): Int\n\
    match result\n\
    when ResultValue.Ok(value) then\n\
        return value\n\
    when ResultValue.Error(_) then\n\
        return 1\n\
    end\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    return consume(ResultValue.Ok(7))\n\
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
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let entry = mir.functions().last().expect("run").symbol();
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering");
    let result = link_with_runtime_and_run(&module, "union-switch");
    assert_eq!(
        result.status.code(),
        Some(7),
        "native executable misexecuted union match: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_preserves_utf8_string_literals_and_value_equality() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
private function same(): Boolean\n\
    return \"Pop 🫧\" == \"Pop 🫧\"\n\
end\n\
private function different(): Boolean\n\
    return \"Pop\" ~= \"Lua\"\n\
end\n\
private function empty(): Boolean\n\
    return \"\" == \"\"\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    if same() and different() and empty() then\n\
        return 42\n\
    else\n\
        return 1\n\
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
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let entry = mir.functions().last().expect("run").symbol();
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering");
    let result = link_with_runtime_and_run(&module, "utf8-string");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted strings: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_preserves_exact_numeric_widths_and_checked_overflow() {
    let success = native_module(
        "namespace Main\n\
private function addByte(left: UInt8, right: UInt8): UInt8\n\
    return left + right\n\
end\n\
private function subtractByte(left: UInt8, right: UInt8): UInt8\n\
    return left - right\n\
end\n\
private function multiplyByte(left: UInt8, right: UInt8): UInt8\n\
    return left * right\n\
end\n\
private function divideByte(left: UInt8, right: UInt8): UInt8\n\
    return left / right\n\
end\n\
private function remainderByte(left: UInt8, right: UInt8): UInt8\n\
    return left % right\n\
end\n\
private function negate(value: Int8): Int8\n\
    return -value\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    if addByte(40, 2) == 42 then\n\
        return 42\n\
    else\n\
        return 1\n\
    end\n\
end\n",
    );
    let text = success.to_string();
    assert!(text.contains("@llvm.uadd.with.overflow.i8"));
    assert!(text.contains("@llvm.usub.with.overflow.i8"));
    assert!(text.contains("@llvm.umul.with.overflow.i8"));
    assert!(text.contains("udiv i8"));
    assert!(text.contains("urem i8"));
    assert!(text.contains("@llvm.ssub.with.overflow.i8"));
    assert!(text.contains("_zero = icmp eq i8"));
    let result = link_with_runtime_and_run(&success, "numeric-success");
    assert_eq!(result.status.code(), Some(42));

    let overflow = native_module(
        "namespace Main\n\
private function addByte(left: UInt8, right: UInt8): UInt8\n\
    return left + right\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    if addByte(255, 1) == 0 then\n\
        return 1\n\
    else\n\
        return 2\n\
    end\n\
end\n",
    );
    let result = link_with_runtime_and_run(&overflow, "numeric-overflow");
    assert!(
        result.status.code().is_none(),
        "checked UInt8 overflow must trap instead of wrapping\n{overflow}"
    );
}

#[test]
fn emitted_llvm_executes_direct_and_nominal_interface_dispatch() {
    let module = native_module(
        "namespace Main\n\
public interface Reader\n\
    function read(value: Int): Int\n\
end\n\
public class IncrementReader implements Reader\n\
    public function IncrementReader:read(value: Int): Int\n\
        return value + 1\n\
    end\n\
end\n\
public class DoubleReader implements Reader\n\
    public function DoubleReader:read(value: Int): Int\n\
        return value + value\n\
    end\n\
end\n\
private function readDirect(reader: IncrementReader): Int\n\
    return reader:read(40)\n\
end\n\
private function readThroughInterface(reader: Reader, value: Int): Int\n\
    return reader:read(value)\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    local reader = IncrementReader {}\n\
    local doubleReader = DoubleReader {}\n\
    if readDirect(reader) == 41 then\n\
        return readThroughInterface(reader, 20) + readThroughInterface(doubleReader, 10) + 1\n\
    else\n\
        return 1\n\
    end\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "interface-dispatch");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted nominal dispatch: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_escaping_mutating_closures() {
    let module = native_module(
        "namespace Main\n\
private function makeCounter(start: Int): function(delta: Int): Int\n\
    local total = start\n\
    return function(delta: Int): Int\n\
        total = total + delta\n\
        return total\n\
    end\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    local counter = makeCounter(1)\n\
    counter(2)\n\
    return counter(39)\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "mutating-closure");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted closure captures: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_direct_function_values_and_recursive_local_functions() {
    let module = native_module(
        "namespace Main\n\
private function increment(value: Int): Int\n\
    return value + 1\n\
end\n\
private function apply(operation: function(value: Int): Int, value: Int): Int\n\
    return operation(value)\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    local function factorial(value: Int): Int\n\
        if value == 0 then\n\
            return 1\n\
        end\n\
        return value * factorial(value - 1)\n\
    end\n\
    return apply(increment, 20) + factorial(3) + 15\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "recursive-closure");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted function values: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_preserves_structural_record_and_tuple_values() {
    let module = native_module(
        "namespace Main\n\
public record Point\n\
    x: Int\n\
    name: String\n\
end\n\
private function aggregates(): Boolean\n\
    local left: Point = { x = 7, name = \"pop\" }\n\
    local reordered: Point = { name = \"pop\", x = 7 }\n\
    local updated = left with { x = 7, }\n\
    return left == reordered and updated == left and (1, \"x\") == (1, \"x\")\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    if aggregates() then\n\
        return 42\n\
    else\n\
        return 1\n\
    end\n\
end\n",
    );
    let result = link_with_runtime_and_run(&module, "aggregate-values");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable misexecuted aggregate values: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_preserves_typed_scalar_aggregate_storage_and_table_entries() {
    let module = native_module(
        "namespace Main\n\
public record Settings\n\
    enabled: Boolean\n\
    small: UInt8\n\
    single: Float32\n\
    wide: Float64\n\
end\n\
public class Box\n\
    private enabled: Boolean\n\
    private small: UInt8\n\
    private single: Float32\n\
    private wide: Float64\n\
    public function Box.new(): Box\n\
        return Box { enabled = true, small = 7, single = 1, wide = 2 }\n\
    end\n\
    public function Box:mutate()\n\
        self.enabled = false\n\
        self.small = 9\n\
        self.single = 3\n\
        self.wide = 4\n\
    end\n\
    public function Box:isValid(): Boolean\n\
        local minimumSingle: Float32 = 2\n\
        local minimumWide: Float64 = 3\n\
        return not self.enabled and self.small == 9 and self.single > minimumSingle and self.wide > minimumWide\n\
    end\n\
end\n\
private function lookup(scores: {[String]: Float32}, key: String): Float32?\n\
    return scores[key]\n\
end\n\
private function aggregates(): Boolean\n\
    local zeroSingle: Float32 = 0\n\
    local minimumWide: Float64 = 1\n\
    local settings: Settings = { enabled = true, small = 7, single = 1, wide = 2 }\n\
    local flags: {Boolean} = { true, false }\n\
    local singles: {Float32} = { 1, 3 }\n\
    local scores: {[String]: Float32} = { first = 1, second = 3 }\n\
    scores[\"third\"] = 5\n\
    local box = Box.new()\n\
    box:mutate()\n\
    return settings.enabled and settings.small == 7 and settings.single > zeroSingle and settings.wide > minimumWide and box:isValid()\n\
end\n\
private function main(): Int\n\
    if aggregates() then\n\
        return 42\n\
    else\n\
        return 1\n\
    end\n\
end\n",
    );
    let text = module.to_string();
    assert!(
        text.contains("call i64 @pop_rt_allocate_table(i64 2, i1 1, i1 0)"),
        "typed tables must use specialized managed-key/scalar-value storage: {text}"
    );
    assert!(
        text.contains("@pop_rt_table_set") && text.contains("@pop_rt_table_get"),
        "typed table access and mutation must use the closed table ABI: {text}"
    );
    let result = link_with_runtime_and_run(&module, "scalar-aggregate-storage");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native executable lost typed scalar aggregate values: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_generic_nominal_iterator_witnesses() {
    let module = native_module(
        "namespace Main\n\
         private class ArrayIterator<T> implements Iterator<T>\n\
             private values: {T}\n\
             private index: Int\n\
             public function ArrayIterator.new(values: {T}): ArrayIterator<T>\n\
                 return ArrayIterator { values = values, index = 1 }\n\
             end\n\
             public function ArrayIterator:iterator(): Iterator<T>\n\
                 return self\n\
             end\n\
             public function ArrayIterator:next(): Iteration<T>\n\
                 if self.index > Array.length(self.values) then\n\
                     return Iteration.End\n\
                 end\n\
                 local value = Array.get(self.values, self.index)\n\
                 self.index += 1\n\
                 return Iteration.Item(value)\n\
             end\n\
         end\n\
         private function main(): Int\n\
             local values: {Int} = {1, 2, 3}\n\
             local iterator: ArrayIterator<Int> = ArrayIterator.new(values)\n\
             local total = 0\n\
             for value in iterator do\n\
                 total += value\n\
             end\n\
             return total\n\
         end\n",
    );
    let result = link_with_runtime_and_run(&module, "generic_nominal_iterator");
    assert_eq!(
        result.status.code(),
        Some(6),
        "native iterator execution failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_generic_user_interface_bound_dispatch() {
    let module = native_module(
        "namespace Main\n\
         private interface Reader<T>\n\
             function read(): T\n\
         end\n\
         private class Box<T> implements Reader<T>\n\
             private value: T\n\
             public function Box.new(value: T): Box<T>\n\
                 return Box { value = value }\n\
             end\n\
             public function Box:read(): T\n\
                 return self.value\n\
             end\n\
         end\n\
         private function readBound<T, TReader: Reader<T>>(reader: TReader): T\n\
             return reader:read()\n\
         end\n\
         private function main(): Int\n\
             local box: Box<Int> = Box.new(42)\n\
             return readBound(box)\n\
         end\n",
    );
    let result = link_with_runtime_and_run(&module, "generic_user_interface");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native generic interface execution failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn emitted_llvm_executes_portable_cross_bubble_generic_capsules() {
    let library_bubble = BubbleId::from_raw(2);
    let library_source = SourceFile::new(
        FileId::from_raw(0),
        "src/generics.pop",
        "namespace Pop.Sequence\n\
         private function privateIdentity<T>(value: T): T\n\
             return value\n\
         end\n\
         public function portableIdentity<T>(value: T): T\n\
             return privateIdentity(value)\n\
         end\n",
    )
    .expect("library source");
    let library = analyze_bubble(FrontEndBubbleInput::new(
        library_bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), library_source)],
    ));
    assert!(library.diagnostics().is_empty());
    let metadata = library
        .reference_metadata()
        .expect("portable metadata")
        .clone();
    let application_source = SourceFile::new(
        FileId::from_raw(1),
        "src/main.pop",
        "namespace Application\n\
         using Pop.Sequence\n\
         private function main(): Int\n\
             return portableIdentity(42)\n\
         end\n",
    )
    .expect("application source");
    let application = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(7),
            NamespaceId::from_raw(7),
            vec![library_bubble],
            vec![FrontEndModule::new(
                ModuleId::from_raw(1),
                application_source,
            )],
        )
        .with_reference_metadata(vec![metadata]),
    );
    assert!(
        application.diagnostics().is_empty(),
        "{}",
        application.diagnostic_snapshot()
    );
    let hir = application.hir().expect("consumer HIR");
    let entry = hir
        .functions()
        .iter()
        .find(|function| function.name() == "main")
        .expect("entry")
        .symbol();
    let mir = lower_hir_bubble(hir, application.types()).expect("specialized MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        application.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering");
    let result = link_with_runtime_and_run(&module, "portable_generic_capsule");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native portable generic execution failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

fn native_module(source_text: &str) -> pop_backend_llvm::LlvmModule {
    native_modules(&[("src/main.pop", source_text)])
}

fn native_modules(sources: &[(&str, &str)]) -> pop_backend_llvm::LlvmModule {
    let modules = sources
        .iter()
        .enumerate()
        .map(|(index, (path, text))| {
            let raw = u32::try_from(index).expect("test Module count");
            FrontEndModule::new(
                ModuleId::from_raw(raw),
                SourceFile::new(FileId::from_raw(raw), *path, *text).expect("source"),
            )
        })
        .collect();
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        modules,
    ));
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let hir = front_end.hir().expect("HIR");
    let entry = hir
        .functions()
        .iter()
        .find(|function| function.name() == "main" && function.type_parameters().is_empty())
        .expect("entry")
        .symbol();
    let mir = lower_hir_bubble(hir, front_end.types()).expect("verified MIR");
    lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering")
}

#[test]
fn checked_numeric_conversions_and_ordered_comparisons_execute_natively() {
    let module = native_module(
        "namespace Main\n\
         private function main(): Int\n\
             local wide: Float64 = Float64(41) + 0.75\n\
             local converted: Int = Int(wide)\n\
             if wide >= 41.75 and wide <= 41.75 then\n\
                 return converted + 1\n\
             end\n\
             return 1\n\
         end\n",
    );
    let text = module.to_string();
    assert!(text.contains("sitofp i64"));
    assert!(text.contains("fptosi double"));
    assert!(text.contains("fcmp oge double"));
    assert!(text.contains("fcmp ole double"));
    let result = link_with_runtime_and_run(&module, "numeric-conversions");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native numeric conversion program failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn typed_string_composition_and_formatting_execute_natively() {
    // ADR 0041: retain runtime operations by formatting parameters in a
    // separate function, then compare the exact UTF-8 bytes natively.
    let module = native_module(
        "namespace Main\n\
         private function describe(count: Int8, ratio: Float32, enabled: Boolean): String\n\
             return `Pop 🫧 {count} {ratio} {enabled}` .. \"!\"\n\
         end\n\
         private function main(): Int\n\
             if describe(-12, 1.5, true) == \"Pop 🫧 -12 1.5 true!\" then\n\
                 return 42\n\
             end\n\
             return 1\n\
         end\n",
    );
    let text = module.to_string();
    assert!(text.contains("@pop_rt_string_concat"));
    assert!(text.contains("@pop_rt_string_format"));
    let result = link_with_runtime_and_run(&module, "string-composition");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native string composition failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn conditional_expressions_and_elseif_execute_lazily_natively() {
    let module = native_module(
        "namespace Main\n\
         private function fail(): Int\n\
             return 1 / 0\n\
         end\n\
         private function main(): Int\n\
             local first = if true then 40 else fail()\n\
             local second = if false then fail() else 1\n\
             if false then\n\
                 return fail()\n\
             elseif first == 40 then\n\
                 return first + second + 1\n\
             else\n\
                 return fail()\n\
             end\n\
         end\n",
    );
    let text = module.to_string();
    assert!(text.contains("br i1"), "{text}");
    let result = link_with_runtime_and_run(&module, "conditional-expression");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native conditional expression failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn compound_assignment_preserves_single_evaluation_natively() {
    let module = native_module(
        "namespace Main\n\
         public class State\n\
             public log: Int = 0\n\
         end\n\
         public class Box\n\
             public value: Int = 10\n\
         end\n\
         private function fieldRight(state: State, box: Box): Int\n\
             state.log = state.log * 10 + 2\n\
             box.value = 20\n\
             return 5\n\
         end\n\
         private function selectArray(state: State, values: {Int}): {Int}\n\
             state.log = state.log * 10 + 3\n\
             return values\n\
         end\n\
         private function selectIndex(state: State): Int\n\
             state.log = state.log * 10 + 4\n\
             return 1\n\
         end\n\
         private function arrayRight(state: State): Int\n\
             state.log = state.log * 10 + 5\n\
             return 4\n\
         end\n\
         private function main(): Int\n\
             local state = State {}\n\
             local box = Box {}\n\
             local values: {Int} = { 2 }\n\
             box.value += fieldRight(state, box)\n\
             selectArray(state, values)[selectIndex(state)] *= arrayRight(state)\n\
             if state.log == 2345 and box.value == 15 and Array.get(values, 1) == 8 then\n\
                 return 42\n\
             end\n\
             return 1\n\
         end\n",
    );
    let result = link_with_runtime_and_run(&module, "compound-assignment");
    assert_eq!(
        result.status.code(),
        Some(42),
        "native compound assignment failed: {}\n{}",
        String::from_utf8_lossy(&result.stderr),
        module
    );
}

#[test]
fn invalid_numeric_conversion_traps_before_native_float_to_integer_lowering() {
    let module = native_module(
        "namespace Main\n\
         private function main(): Int\n\
             local invalid: Byte = Byte(256.0)\n\
             return Int(invalid)\n\
         end\n",
    );
    let text = module.to_string();
    assert!(text.contains("call double @llvm.trunc.f64"));
    assert!(text.contains("call void @pop_rt_trap()"));
    assert!(text.contains("fptoui double"));
    let result = link_with_runtime_and_run(&module, "numeric-conversion-trap");
    assert!(
        !result.status.success(),
        "invalid conversion must trap\n{module}"
    );
}

#[test]
fn every_numeric_conversion_family_emits_valid_llvm() {
    let module = native_module(&numeric_conversion_matrix_source());
    let input = std::env::temp_dir().join("pop-backend-llvm-numeric-conversion-matrix.ll");
    let output = std::env::temp_dir().join("pop-backend-llvm-numeric-conversion-matrix.bc");
    fs::write(&input, module.to_string()).expect("write numeric conversion LLVM");
    let assembled = Command::new("llvm-as")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("llvm-as must be installed");
    assert!(
        assembled.status.success(),
        "llvm-as rejected numeric conversion matrix: {}\n{}",
        String::from_utf8_lossy(&assembled.stderr),
        module
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
}

fn numeric_conversion_matrix_source() -> String {
    const INTEGERS: [&str; 8] = [
        "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64",
    ];
    const FLOATS: [&str; 2] = ["Float32", "Float64"];
    let mut source = String::from("namespace Main\n");
    for source_type in INTEGERS.into_iter().chain(FLOATS) {
        for target_type in INTEGERS.into_iter().chain(FLOATS) {
            let name = format!("convert{source_type}To{target_type}");
            writeln!(
                source,
                "private function {name}(value: {source_type}): {target_type}\n    return {target_type}(value)\nend"
            )
            .expect("source text");
        }
    }
    source.push_str("private function main(): Int\n    return 0\nend\n");
    source
}

fn link_with_runtime_and_run(module: &pop_backend_llvm::LlvmModule, name: &str) -> Output {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("backend crate is under the repository root");
    let build = Command::new("cargo")
        .current_dir(root)
        .args(["build", "-p", "pop-runtime-native"])
        .output()
        .expect("cargo must be available");
    assert!(
        build.status.success(),
        "runtime build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let input = std::env::temp_dir().join(format!("pop-backend-llvm-{name}.ll"));
    let executable = std::env::temp_dir().join(format!("pop-backend-llvm-{name}"));
    fs::write(&input, module.to_string()).expect("write temporary LLVM input");
    let link = Command::new("clang")
        .arg(&input)
        .arg(root.join("target/debug/libpop_runtime_native.a"))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("clang must be installed");
    assert!(
        link.status.success(),
        "clang rejected LLVM: {}\n{}",
        String::from_utf8_lossy(&link.stderr),
        module
    );
    let result = Command::new(&executable)
        .output()
        .expect("native executable runs");
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(executable);
    result
}

fn link_with_forced_relocation_runtime_and_run(llvm: &str, name: &str) -> Output {
    let input = std::env::temp_dir().join(format!("pop-backend-llvm-{name}.ll"));
    let runtime = std::env::temp_dir().join(format!("pop-backend-llvm-{name}-runtime.c"));
    let executable = std::env::temp_dir().join(format!("pop-backend-llvm-{name}"));
    fs::write(&input, llvm).expect("write forced-relocation LLVM input");
    fs::write(
        &runtime,
        concat!(
            "#include <stdint.h>\n",
            "#include <stdlib.h>\n",
            "static uint64_t current_token;\n",
            "uint8_t pop_rt_supports_abi(uint16_t major, uint16_t minor) {\n",
            "  return major == 2 && minor == 0;\n",
            "}\n",
            "uint64_t pop_rt_allocate_array(uint64_t length, uint8_t references) {\n",
            "  (void)length; (void)references; current_token = 41; return current_token;\n",
            "}\n",
            "uint8_t pop_rt_gc_safe_point_v2(uint32_t point, uint64_t *roots, uint64_t count) {\n",
            "  (void)point;\n",
            "  for (uint64_t index = 0; index < count; ++index) {\n",
            "    if (roots[index] != current_token) abort();\n",
            "    current_token += 100; roots[index] = current_token;\n",
            "  }\n",
            "  return 1;\n",
            "}\n",
            "uint64_t pop_rt_retain_root(uint64_t token) {\n",
            "  if (token != current_token) abort(); return token;\n",
            "}\n",
            "uint8_t pop_rt_release_root(uint64_t token) {\n",
            "  if (token != current_token) abort(); return 1;\n",
            "}\n",
            "void pop_rt_trap(void) { abort(); }\n",
        ),
    )
    .expect("write forced-relocation native runtime");
    let link = Command::new("clang")
        .args(["-O3", "-Werror", "-Wno-override-module"])
        .arg(&input)
        .arg(&runtime)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("clang must be installed");
    assert!(
        link.status.success(),
        "clang rejected forced-relocation LLVM: {}\n{}",
        String::from_utf8_lossy(&link.stderr),
        llvm
    );
    let result = Command::new(&executable)
        .output()
        .expect("forced-relocation executable runs");
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(runtime);
    let _ = fs::remove_file(executable);
    result
}

#[test]
fn emitted_llvm_can_link_against_the_rust_bootstrap_runtime() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("backend crate is under the repository root");
    let build = Command::new("cargo")
        .current_dir(root)
        .args(["build", "-p", "pop-runtime-native"])
        .output()
        .expect("cargo must be available");
    assert!(
        build.status.success(),
        "runtime build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let source = std::env::temp_dir().join("pop-runtime-link.ll");
    let executable = std::env::temp_dir().join("pop-runtime-link");
    fs::write(
        &source,
        concat!(
            "target triple = \"x86_64-unknown-linux-gnu\"\n",
            "declare i64 @pop_rt_allocate_array(i64, i1)\n",
            "declare i8 @pop_rt_array_set(i64, i64, i64)\n",
            "declare i64 @pop_rt_array_get(i64, i64)\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %handle = call i64 @pop_rt_allocate_array(i64 2, i1 0)\n",
            "  %stored = call i8 @pop_rt_array_set(i64 %handle, i64 1, i64 41)\n",
            "  %value = call i64 @pop_rt_array_get(i64 %handle, i64 1)\n",
            "  %valid_handle = icmp ne i64 %handle, 0\n",
            "  %valid_store = icmp eq i8 %stored, 1\n",
            "  %valid_value = icmp eq i64 %value, 41\n",
            "  %valid_store_and_handle = and i1 %valid_handle, %valid_store\n",
            "  %valid = and i1 %valid_store_and_handle, %valid_value\n",
            "  %code = zext i1 %valid to i32\n",
            "  ret i32 %code\n",
            "}\n"
        ),
    )
    .expect("write runtime-link LLVM input");
    let library = root.join("target/debug/libpop_runtime_native.a");
    let link = Command::new("clang")
        .current_dir(root)
        .arg(&source)
        .arg(&library)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("clang must be installed");
    assert!(
        link.status.success(),
        "native runtime link failed: {}",
        String::from_utf8_lossy(&link.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("linked runtime executable must run");
    assert_eq!(run.status.code(), Some(1));
    let _ = fs::remove_file(source);
    let _ = fs::remove_file(executable);
}

#[test]
fn allocating_and_calling_mir_modules_are_accepted_by_llvm_as() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
public function add(left: Int, right: Int): Int\n\
    return left + right\n\
end\n\
public function run(): {Int}\n\
    local pair: (Int, Int) = (1, 2)\n\
    local values: {Int} = { add(1, 2) }\n\
    return values\n\
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
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let text = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default(),
    )
    .expect("LLVM lowering")
    .to_string();
    assert!(!text.contains("semantic_"));
    assert!(text.contains("pop_rt_array_set"));
    let input = std::env::temp_dir().join("pop-backend-llvm-allocations.ll");
    let output = std::env::temp_dir().join("pop-backend-llvm-allocations.bc");
    fs::write(&input, text).expect("write temporary LLVM input");
    let result = Command::new("llvm-as")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("llvm-as must be installed");
    assert!(
        result.status.success(),
        "llvm-as rejected allocation/call IR: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
}

#[test]
fn class_field_operations_lower_to_layouted_runtime_calls() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
public class Box\n\
    public value: Int\n\
    public function Box.new(value: Int): Box\n\
        return Box { value = value }\n\
    end\n\
end\n\
private function main(arguments: Array<String>): Int\n\
    local box = Box.new(41)\n\
    return box.value\n\
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
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let text = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default(),
    )
    .expect("LLVM lowering")
    .to_string();
    assert!(text.contains("pop_rt_field_set"));
    assert!(text.contains("pop_rt_field_get"));
    let input = std::env::temp_dir().join("pop-backend-llvm-class.ll");
    let output = std::env::temp_dir().join("pop-backend-llvm-class.bc");
    fs::write(&input, text).expect("write class LLVM input");
    let assembled = Command::new("llvm-as")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("llvm-as must be installed");
    assert!(
        assembled.status.success(),
        "llvm-as rejected class IR: {}",
        String::from_utf8_lossy(&assembled.stderr)
    );
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("backend crate is under the repository root");
    let build = Command::new("cargo")
        .current_dir(root)
        .args(["build", "-p", "pop-runtime-native"])
        .output()
        .expect("cargo must be available");
    assert!(build.status.success());
    let executable = std::env::temp_dir().join("pop-backend-llvm-class");
    let input = std::env::temp_dir().join("pop-backend-llvm-class-execution.ll");
    let output = std::env::temp_dir().join("pop-backend-llvm-class-execution.bc");
    let class_text = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect("LLVM lowering")
    .to_string();
    fs::write(&input, class_text).expect("write executable class IR");
    let link = Command::new("clang")
        .current_dir(root)
        .arg(&input)
        .arg(root.join("target/debug/libpop_runtime_native.a"))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("clang must be installed");
    assert!(
        link.status.success(),
        "class runtime link failed: {}",
        String::from_utf8_lossy(&link.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("class executable runs");
    assert_eq!(run.status.code(), Some(41));
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
    let _ = fs::remove_file(executable);
}

#[test]
fn backend_rejects_unverified_mir_instead_of_emitting_partial_llvm() {
    let mir = pop_mir::parse_mir_dump(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[]\n  b0():\n    missing\n",
    )
    .expect("fixture parses");
    let arena = pop_types::TypeArena::new();
    let error = lower_mir_to_llvm_ir(&mir, &arena, &target(), LlvmLoweringOptions::default())
        .expect_err("invalid MIR must be rejected");
    assert!(error.to_string().contains("MIR verification"));
}

#[test]
fn typed_results_errors_and_cleanup_lower_without_backend_semantic_fallback() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/errors.pop",
        "namespace Main\n\
         --- <summary>\n\
         --- Describes loading failures.\n\
         --- </summary>\n\
         public error LoadError\n\
             --- <summary>\n\
             --- Loading failed.\n\
             --- </summary>\n\
             Failed\n\
         end\n\
         private function fail(): Result<Int, LoadError>\n\
             return Result.Error(LoadError.Failed())\n\
         end\n\
         --- <error type=\"LoadError.Failed\">\n\
         --- Loading failed.\n\
         --- </error>\n\
         public function forward(): Result<Int, LoadError>\n\
             defer\n\
                 print(\"cleanup\")\n\
             end\n\
             local invoke = fail\n\
             local value = try invoke()\n\
             return Result.Ok(value)\n\
         end\n\
         public function describe(error: LoadError): String\n\
             match error\n\
             when LoadError.Failed then\n\
                 return \"failed\"\n\
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
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let llvm = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default(),
    )
    .expect("LLVM lowering")
    .to_string();

    assert!(llvm.contains("icmp eq i64"), "{llvm}");
    assert!(llvm.contains("@pop_rt_field_get"), "{llvm}");
    assert!(llvm.contains("@pop_rt_field_set"), "{llvm}");
    assert!(llvm.contains("switch i64"), "{llvm}");
    assert!(llvm.contains("@pop_rt_continue_unwind"), "{llvm}");
    assert!(!llvm.to_ascii_lowercase().contains("dynamic"), "{llvm}");
}

#[test]
fn typed_result_failure_runs_managed_cleanup_in_native_execution() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/nativeErrors.pop",
        "namespace Main\n\
         private error LoadError\n\
             Failed\n\
         end\n\
         private class Marker\n\
             public count: Int = 0\n\
         end\n\
         private function fail(): Result<Int, LoadError>\n\
             return Result.Error(LoadError.Failed())\n\
         end\n\
         private function forward(marker: Marker): Result<Int, LoadError>\n\
             defer\n\
                 marker.count = marker.count + 1\n\
             end\n\
             local value = try fail()\n\
             return Result.Ok(value)\n\
         end\n\
         private function main(): Int\n\
             local marker = Marker {}\n\
             local result = forward(marker)\n\
             match result\n\
             when Result.Ok(value) then\n\
                 return value\n\
             when Result.Error(error) then\n\
                 match error\n\
                 when LoadError.Failed then\n\
                     return marker.count\n\
                 end\n\
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
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let hir = front_end.hir().expect("HIR");
    let entry = hir
        .functions()
        .iter()
        .find(|function| function.name() == "main")
        .expect("entry")
        .symbol();
    let mir = lower_hir_bubble(hir, front_end.types()).expect("verified MIR");
    let module = lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering");

    let result = link_with_runtime_and_run(&module, "typed-result-cleanup");
    assert_eq!(result.status.code(), Some(1), "{module}");
}
