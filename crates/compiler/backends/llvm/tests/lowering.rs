use pop_backend_llvm::{LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::lower_hir_bubble;
use pop_source::SourceFile;
use pop_target::{Endianness, PointerWidth, TargetSpec};
use std::fs;
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
        !text.contains("pop_rt_semantic"),
        "runtime operations must use closed PLRI identities"
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

fn native_module(source_text: &str) -> pop_backend_llvm::LlvmModule {
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", source_text).expect("source");
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
    let entry = mir.functions().last().expect("entry").symbol();
    lower_mir_to_llvm_ir(
        &mir,
        front_end.types(),
        &target(),
        LlvmLoweringOptions::default().with_entry_point(entry),
    )
    .expect("LLVM lowering")
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
