use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use pop_backend_c::{CBackendError, CLoweringOptions, lower_mir_to_c};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, BuiltinTypeId, FileId, ModuleId, NamespaceId};
use pop_mir::{MirDeclarationKind, lower_hir_bubble, optimize_mir, parse_mir_dump};
use pop_source::SourceFile;
use pop_types::SemanticType;

fn lower(source_text: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", source_text).expect("source");
    let front_end = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        )
        .with_implicit_main_entry(ModuleId::from_raw(0)),
    );
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    let optimized = optimize_mir(mir, front_end.types()).expect("optimized MIR");
    (optimized, front_end.types().clone())
}

fn lower_ffi(source_text: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    let ffi = BubbleId::from_raw(20);
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", source_text).expect("source");
    let front_end = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir =
        lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("verified MIR");
    (mir, front_end.types().clone())
}

#[test]
fn experimental_c_backend_rejects_callback_operations_without_a_fallback() {
    let (mir, types) = lower_ffi(
        "namespace CallbackDemo\n\
         private type CallbackSignature = function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int\n\
         public function closeCallback(callback: Ffi.RegisteredCallback<CallbackSignature>): Result<nil, Ffi.CallbackInUseError>\n\
             return Ffi.Callback.close(callback)\n\
         end\n",
    );

    assert!(matches!(
        lower_mir_to_c(&mir, &types, CLoweringOptions::default()),
        Err(CBackendError::UnsupportedInstruction { .. })
    ));
}

#[test]
fn experimental_c_backend_rejects_complete_view_mir_before_emission() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function inspect(bytes: Bytes): Int\n\
             local whole = Bytes.view(bytes)\n\
             local part = Bytes.slice(whole, 1, 1)\n\
             local present: Byte? = Bytes.get(part, 1)\n\
             local copy = Bytes.toBytes(part)\n\
             return Bytes.length(Bytes.view(copy))\n\
         end\n",
    );
    let dump = mir.dump();
    for operation in [
        "viewCreate",
        "viewSlice",
        "viewLength",
        "viewGetByte",
        "viewMaterialize",
        "viewEnd",
    ] {
        assert!(dump.contains(operation), "missing {operation}:\n{dump}");
    }

    assert!(matches!(
        lower_mir_to_c(&mir, &types, CLoweringOptions::default()),
        Err(CBackendError::UnsupportedInstruction { .. })
    ));
}

#[test]
fn experimental_c_backend_rejects_generated_codec_operations_without_a_fallback() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/codec.pop",
        "namespace Example.Models\n\
         @RetainMetadata(use = Metadata.Use.Codec, schemaVersion = 1)\n\
         public record Payload\n\
             age: UInt32\n\
         end\n\
         public function schema(): Codec.Schema<Payload>\n\
             return PayloadSchema\n\
         end\n",
    )
    .expect("codec source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(7),
        NamespaceId::from_raw(7),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let base = lower_hir_bubble(front_end.hir().expect("codec HIR"), front_end.types())
        .expect("codec MIR");
    let adapter = base.generated_codec_adapters()[0].symbol();
    let target_type = base
        .declarations()
        .iter()
        .find_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Record(record) => Some(record.type_id()),
            _ => None,
        })
        .expect("retained record type");
    let mut types = front_end.types().clone();
    let writer = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(119),
            arguments: Vec::new(),
        })
        .expect("Codec.Writer");
    let reader = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(120),
            arguments: Vec::new(),
        })
        .expect("Codec.Reader");
    let error = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(121),
            arguments: Vec::new(),
        })
        .expect("Codec.Error");
    let nil = types.source_type("nil").expect("nil");
    let encode_result = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(100),
            arguments: vec![nil, error],
        })
        .expect("encode Result");
    let decode_result = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(100),
            arguments: vec![target_type, error],
        })
        .expect("decode Result");
    let mut dump = base.dump();
    write!(
        dump,
        "function s900 f900(t{}, t{}) -> (t{}) effects[Allocates,GcSafePoint,Roots]\n  b0(v0:t{}, v1:t{}):\n    do v2 gcSafePoint sp0 roots (v1)\n    v3:t{} = codecEncode s{} v0 v1 result bt100 success resultCase#0 failure resultCase#1\n    return (v3)\n\
         function s901 f901(t{}) -> (t{}) effects[Allocates,GcSafePoint,Roots]\n  b0(v0:t{}):\n    do v1 gcSafePoint sp1 roots (v0)\n    v2:t{} = codecDecode s{} v0 result bt100 success resultCase#0 failure resultCase#1\n    return (v2)\n",
        target_type.raw(),
        writer.raw(),
        encode_result.raw(),
        target_type.raw(),
        writer.raw(),
        encode_result.raw(),
        adapter.raw(),
        reader.raw(),
        decode_result.raw(),
        reader.raw(),
        decode_result.raw(),
        adapter.raw(),
    )
    .expect("append canonical codec operations");
    let mir = parse_mir_dump(&dump).expect("verified generated-codec MIR");
    assert!(dump.contains("codecEncode"), "{dump}");
    assert!(dump.contains("codecDecode"), "{dump}");

    assert!(matches!(
        lower_mir_to_c(&mir, &types, CLoweringOptions::default()),
        Err(CBackendError::UnsupportedInstruction { .. })
    ));
}

#[test]
fn emits_deterministic_strict_c11_with_checked_direct_calls_and_entry() {
    let (mir, types) = lower(
        "namespace Main\n\
         private function add(left: Int, right: Int): Int\n\
             return left + right\n\
         end\n\
         function main(): Int\n\
             return add(19, 23)\n\
         end\n",
    );
    let entry = mir.functions()[1].symbol();
    let options = CLoweringOptions::default().with_entry_point(entry);
    let first = lower_mir_to_c(&mir, &types, options).expect("C lowering");
    let second = lower_mir_to_c(&mir, &types, options).expect("deterministic C lowering");

    assert_eq!(first.as_str(), second.as_str());
    assert!(first.as_str().starts_with("/* Generated by Pop Lang"));
    assert!(first.as_str().contains("#include <stdint.h>"));
    assert!(first.as_str().contains("static int64_t pop_b0_s0("));
    assert!(first.as_str().contains("pop_checked_add_i64"));
    assert!(first.as_str().contains("int main(void)"));
    assert!(!first.as_str().contains("function add"));
    assert!(!first.as_str().contains("private function"));

    let root = temporary_root("executes");
    let source_path = root.with_extension("c");
    let executable_path = root.with_extension("out");
    std::fs::write(&source_path, first.as_str()).expect("write generated C");
    let compiler = Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
        ])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable_path)
        .output()
        .expect("C compiler runs");
    assert!(
        compiler.status.success(),
        "generated C did not compile:\n{}\n{}",
        String::from_utf8_lossy(&compiler.stderr),
        first.as_str()
    );
    let execution = Command::new(&executable_path)
        .output()
        .expect("generated C runs");
    assert_eq!(execution.status.code(), Some(42));
    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(executable_path);
}

#[test]
fn runtime_free_c_rejects_async_functions_without_sync_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         private async function work(): Int\n\
             return 42\n\
         end\n",
    );

    assert!(matches!(
        lower_mir_to_c(&mir, &types, CLoweringOptions::default()),
        Err(CBackendError::UnsupportedAsync(_))
    ));
}

#[test]
fn checked_numeric_conversions_and_complete_ordering_execute_as_strict_c11() {
    let (mir, types) = lower(
        "namespace Main\n\
         private function convert(value: Int): Int\n\
             local wide: Float64 = Float64(value) + 0.75\n\
             local converted: Int = Int(wide)\n\
             if wide >= 41.75 and wide <= 41.75 then\n\
                 return converted\n\
             end\n\
             return 0\n\
         end\n\
         function main(): Int\n\
             return convert(41) + 1\n\
         end\n",
    );
    let entry = mir.functions()[1].symbol();
    let translation = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(entry),
    )
    .expect("numeric C lowering");
    assert!(translation.as_str().contains("(long double)"));
    assert!(translation.as_str().contains(">="));
    assert!(translation.as_str().contains("<="));

    let root = temporary_root("numeric-conversions");
    let source_path = root.with_extension("c");
    let executable_path = root.with_extension("out");
    std::fs::write(&source_path, translation.as_str()).expect("write generated C");
    let compiler = Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
        ])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable_path)
        .output()
        .expect("C compiler runs");
    assert!(
        compiler.status.success(),
        "generated numeric C did not compile:\n{}\n{}",
        String::from_utf8_lossy(&compiler.stderr),
        translation.as_str()
    );
    let execution = Command::new(&executable_path)
        .output()
        .expect("generated C runs");
    assert_eq!(execution.status.code(), Some(42));
    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(executable_path);
}

#[test]
fn invalid_numeric_conversion_traps_in_strict_c11_output() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             local invalid: Byte = Byte(256.0)\n\
             return Int(invalid)\n\
         end\n",
    );
    let entry = mir.functions()[0].symbol();
    let translation = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(entry),
    )
    .expect("trapping numeric C lowering");
    let root = temporary_root("numeric-conversion-trap");
    let source_path = root.with_extension("c");
    let executable_path = root.with_extension("out");
    std::fs::write(&source_path, translation.as_str()).expect("write generated C");
    let compiler = Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
        ])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable_path)
        .output()
        .expect("C compiler runs");
    assert!(
        compiler.status.success(),
        "generated trapping C did not compile:\n{}\n{}",
        String::from_utf8_lossy(&compiler.stderr),
        translation.as_str()
    );
    let execution = Command::new(&executable_path)
        .output()
        .expect("generated C runs");
    assert!(!execution.status.success(), "invalid conversion must trap");
    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(executable_path);
}

#[test]
fn every_numeric_conversion_family_emits_warning_free_strict_c11() {
    for source_type in [
        "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float32",
        "Float64",
    ] {
        let source = numeric_conversion_matrix_source(source_type);
        let (mir, types) = lower(&source);
        let entry = mir.functions().last().expect("main").symbol();
        let translation = lower_mir_to_c(
            &mir,
            &types,
            CLoweringOptions::default().with_entry_point(entry),
        )
        .expect("numeric conversion matrix C lowering");
        let root = temporary_root(&format!("numeric-conversion-matrix-{source_type}"));
        let source_path = root.with_extension("c");
        let executable_path = root.with_extension("out");
        std::fs::write(&source_path, translation.as_str()).expect("write generated C");
        let compiler = Command::new("cc")
            .args([
                "-std=c11",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic",
            ])
            .arg(&source_path)
            .arg("-o")
            .arg(&executable_path)
            .output()
            .expect("C compiler runs");
        assert!(
            compiler.status.success(),
            "{source_type} conversion matrix did not compile:\n{}\n{}",
            String::from_utf8_lossy(&compiler.stderr),
            translation.as_str()
        );
        let execution = Command::new(&executable_path)
            .output()
            .expect("generated C runs");
        assert_eq!(execution.status.code(), Some(0), "{source_type}");
        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(executable_path);
    }
}

fn numeric_conversion_matrix_source(source_type: &str) -> String {
    const INTEGERS: [&str; 8] = [
        "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64",
    ];
    const FLOATS: [&str; 2] = ["Float32", "Float64"];
    let mut source = String::from("namespace Main\n");
    let mut calls = Vec::new();
    for target_type in INTEGERS.into_iter().chain(FLOATS) {
        let name = format!("convert{source_type}To{target_type}");
        writeln!(
            source,
            "private function {name}(value: {source_type}): {target_type}\n    return {target_type}(value)\nend"
        )
        .expect("source text");
        calls.push(format!("    total = total + Int({name}(0))\n"));
    }
    source.push_str("function main(): Int\n    local total: Int = 0\n");
    for call in calls {
        source.push_str(&call);
    }
    source.push_str("    return total\nend\n");
    source
}

#[test]
fn rejects_managed_declarations_without_emitting_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         public class Box\n\
             public value: Int\n\
         end\n\
         function main(): Int\n\
             return 0\n\
         end\n",
    );
    let entry = mir.functions()[0].symbol();
    let error = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(entry),
    )
    .expect_err("runtime-free C backend must reject managed declarations");

    assert!(matches!(error, CBackendError::UnsupportedDeclarations));
    assert!(error.to_string().contains("requires the Pop runtime"));
}

#[test]
fn rejects_checked_downcast_before_managed_declaration_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         public interface Reader\n\
             function read(): Int\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(): Int\n\
                 return 1\n\
             end\n\
         end\n\
         private function cast(reader: Reader): FileReader?\n\
             return FileReader(reader)\n\
         end\n\
         function main(): Int\n\
             return 0\n\
         end\n",
    );

    assert!(matches!(
        lower_mir_to_c(&mir, &types, CLoweringOptions::default()),
        Err(CBackendError::UnsupportedInstruction { .. })
    ));
}

#[test]
fn rejects_first_class_ranges_without_emitting_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             local total = 0\n\
             for value in Range.create(1, 3) do\n\
                 total += value\n\
             end\n\
             return total\n\
         end\n",
    );
    let entry = mir.functions()[0].symbol();
    assert!(matches!(
        lower_mir_to_c(
            &mir,
            &types,
            CLoweringOptions::default().with_entry_point(entry),
        ),
        Err(CBackendError::UnsupportedInstruction { .. })
    ));
}

#[test]
fn rejects_typed_results_errors_and_cleanup_without_emitting_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         private error LoadError\n\
             Failed\n\
         end\n\
         private function fail(): Result<Int, LoadError>\n\
             defer\n\
                 print(\"cleanup\")\n\
             end\n\
             return Result.Error(LoadError.Failed())\n\
         end\n\
         function main(): Int\n\
             local result = fail()\n\
             match result\n\
             when Result.Ok(value) then\n\
                 return value\n\
             when Result.Error(error) then\n\
                 match error\n\
                 when LoadError.Failed then\n\
                     return 1\n\
                 end\n\
             end\n\
         end\n",
    );
    let error = lower_mir_to_c(&mir, &types, CLoweringOptions::default())
        .expect_err("experimental C must reject typed error declarations");

    assert!(matches!(error, CBackendError::UnsupportedDeclarations));
}

#[test]
fn rejects_specialized_generic_data_without_emitting_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         private record Box<T>\n\
             value: T\n\
         end\n\
         private function boxed<T>(value: T): Box<T>\n\
             local result: Box<T> = { value = value }\n\
             return result\n\
         end\n\
         function main(): Int\n\
             local value: Box<Int> = boxed<<Int>>(7)\n\
             return value.value\n\
         end\n",
    );
    let entry = mir
        .functions()
        .iter()
        .find(|function| function.parameters().is_empty())
        .expect("entry")
        .symbol();
    let error = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(entry),
    )
    .expect_err("experimental C must remain fail-closed for generic data");

    assert!(matches!(error, CBackendError::UnsupportedDeclarations));
}

#[test]
fn typed_integer_and_literal_string_output_use_safe_c_adapters() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main()\n\
             print(42)\n\
             print(\"Teste\")\n\
         end\n",
    );
    let entry = mir.functions()[0].symbol();
    let translation = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(entry),
    )
    .expect("typed output C lowering");
    assert!(!translation.as_str().contains("Teste"));
    assert!(translation.as_str().contains("UINT8_C(0x54)"));

    let root = temporary_root("typed-output");
    let source_path = root.with_extension("c");
    let executable_path = root.with_extension("out");
    std::fs::write(&source_path, translation.as_str()).expect("write generated C");
    let compiler = Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
        ])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable_path)
        .output()
        .expect("C compiler runs");
    assert!(
        compiler.status.success(),
        "generated output C did not compile:\n{}\n{}",
        String::from_utf8_lossy(&compiler.stderr),
        translation.as_str()
    );
    let execution = Command::new(&executable_path)
        .output()
        .expect("generated output C runs");
    assert!(execution.status.success());
    assert_eq!(execution.stdout, b"42\nTeste\n");
    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(executable_path);
}

#[test]
fn constant_interpolation_and_concatenation_execute_as_strict_c11() {
    // ADR 0041: the runtime-free C slice accepts composition after portable
    // MIR proves and folds every segment to one UTF-8 literal.
    let (mir, types) = lower(
        "namespace Main\n\
         function main()\n\
             print(`Pop 🫧 {12} {1.5} {true}` .. \"!\")\n\
         end\n",
    );
    let entry = mir.functions()[0].symbol();
    let translation = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(entry),
    )
    .expect("constant string C lowering");
    assert!(!translation.as_str().contains("string.format"));
    assert!(!translation.as_str().contains("string.concat"));

    let root = temporary_root("constant-string-composition");
    let source_path = root.with_extension("c");
    let executable_path = root.with_extension("out");
    std::fs::write(&source_path, translation.as_str()).expect("write generated C");
    let compiler = Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
        ])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable_path)
        .output()
        .expect("C compiler runs");
    assert!(
        compiler.status.success(),
        "generated string C did not compile:\n{}\n{}",
        String::from_utf8_lossy(&compiler.stderr),
        translation.as_str()
    );
    let execution = Command::new(&executable_path)
        .output()
        .expect("generated C runs");
    assert!(execution.status.success());
    assert_eq!(execution.stdout, "Pop 🫧 12 1.5 true!\n".as_bytes());
    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(executable_path);
}

#[test]
fn strict_c11_preserves_block_arguments_and_conditional_control_flow() {
    let (mir, types) = lower(
        "namespace Main\n\
         private function choose(enabled: Boolean, left: Int, right: Int): Int\n\
             local value = left\n\
             if enabled then\n\
                 value = right\n\
             end\n\
             return value\n\
         end\n\
         function main(): Int\n\
             return choose(true, 1, 42)\n\
         end\n",
    );
    let entry = mir.functions()[1].symbol();
    let translation = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(entry),
    )
    .expect("C lowering");

    let root = temporary_root("control-flow");
    let source_path = root.with_extension("c");
    let executable_path = root.with_extension("out");
    std::fs::write(&source_path, translation.as_str()).expect("write generated C");
    let compiler = Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
        ])
        .arg(&source_path)
        .arg("-o")
        .arg(&executable_path)
        .output()
        .expect("C compiler runs");
    assert!(
        compiler.status.success(),
        "generated control-flow C did not compile:\n{}\n{}",
        String::from_utf8_lossy(&compiler.stderr),
        translation.as_str()
    );
    let execution = Command::new(&executable_path)
        .output()
        .expect("generated C runs");
    assert_eq!(execution.status.code(), Some(42));
    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(executable_path);
}

#[test]
fn runtime_free_c_rejects_repeat_until_safe_points_without_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             local value = 0\n\
             repeat\n\
                 value = value + 1\n\
             until value == 42\n\
             return value\n\
         end\n",
    );
    let error = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect_err("runtime-free C cannot erase a repeat-until safe point");
    assert!(matches!(
        error,
        CBackendError::UnsupportedInstruction { .. }
    ));
}

#[test]
fn runtime_free_c_rejects_optional_flow_without_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function choose(value: Int?, fallback: Int): Int\n\
             return value ?? fallback\n\
         end\n",
    );
    let error = lower_mir_to_c(&mir, &types, CLoweringOptions::default())
        .expect_err("runtime-free C has no optional representation");
    assert!(matches!(error, CBackendError::UnsupportedType(_)));
}

#[test]
fn runtime_free_c_rejects_numeric_range_safe_points_without_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             local total = 0\n\
             for index = 1, 42 do\n\
                 total = total + index\n\
             end\n\
             return total\n\
         end\n",
    );
    let error = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
    )
    .expect_err("runtime-free C cannot erase a numeric-range safe point");
    assert!(matches!(
        error,
        CBackendError::UnsupportedInstruction { .. }
    ));
}

#[test]
fn runtime_free_c_rejects_nominal_iteration_without_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             local values: {Int} = { 20, 22 }\n\
             local total = 0\n\
             for value in values do\n\
                 total = total + value\n\
             end\n\
             return total\n\
         end\n",
    );

    assert!(matches!(
        lower_mir_to_c(
            &mir,
            &types,
            CLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
        ),
        Err(CBackendError::UnsupportedInstruction { .. })
    ));
}

#[test]
fn runtime_free_c_rejects_specialized_nominal_iterator_witnesses() {
    let (mir, types) = lower(
        "namespace Main\n\
         private class Once<T> implements Iterator<T>\n\
             private value: T\n\
             private finished: Boolean\n\
             public function Once.new(value: T): Once<T>\n\
                 return Once { value = value, finished = false }\n\
             end\n\
             public function Once:iterator(): Iterator<T>\n\
                 return self\n\
             end\n\
             public function Once:next(): Iteration<T>\n\
                 if self.finished then\n\
                     return Iteration.End\n\
                 end\n\
                 self.finished = true\n\
                 return Iteration.Item(self.value)\n\
             end\n\
         end\n\
         function main(): Int\n\
             local iterator: Once<Int> = Once.new(42)\n\
             for value in iterator do\n\
                 return value\n\
             end\n\
             return 0\n\
         end\n",
    );

    assert!(matches!(
        lower_mir_to_c(
            &mir,
            &types,
            CLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
        ),
        Err(CBackendError::UnsupportedDeclarations)
    ));
}

#[test]
fn runtime_free_c_rejects_growable_lists_without_a_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         function main(): Int\n\
             local values = List.create<<Int>>()\n\
             List.add(values, 42)\n\
             return List.get(values, 1)\n\
         end\n",
    );
    assert!(matches!(
        lower_mir_to_c(
            &mir,
            &types,
            CLoweringOptions::default().with_entry_point(mir.functions()[0].symbol()),
        ),
        Err(CBackendError::UnsupportedInstruction { .. })
    ));
}

#[test]
fn runtime_free_c_rejects_fixed_packs_without_a_dynamic_fallback() {
    let (mir, types) = lower(
        "namespace Main\n\
         private function split(value: Int): (Int, Int)\n\
             return value, value + 1\n\
         end\n\
         function main(): Int\n\
             local left, right = split(20)\n\
             return left + right\n\
         end\n",
    );
    let error = lower_mir_to_c(
        &mir,
        &types,
        CLoweringOptions::default().with_entry_point(mir.functions()[1].symbol()),
    )
    .expect_err("runtime-free C cannot represent a managed fixed pack");
    assert!(matches!(error, CBackendError::UnsupportedType(_)));
}

#[test]
fn checked_addition_traps_for_every_fixed_integer_width() {
    for (name, maximum) in [
        ("Int8", "127"),
        ("Int16", "32767"),
        ("Int32", "2147483647"),
        ("Int64", "9223372036854775807"),
        ("UInt8", "255"),
        ("UInt16", "65535"),
        ("UInt32", "4294967295"),
        ("UInt64", "18446744073709551615"),
    ] {
        let source = format!(
            "namespace Main\n\
             private function overflow(value: {name}): {name}\n\
                 return value + 1\n\
             end\n\
             function main(): Int\n\
                 overflow({maximum})\n\
                 return 0\n\
             end\n"
        );
        let (mir, types) = lower(&source);
        let entry = mir.functions()[1].symbol();
        let translation = lower_mir_to_c(
            &mir,
            &types,
            CLoweringOptions::default().with_entry_point(entry),
        )
        .expect("C lowering");
        let root = temporary_root(name);
        let source_path = root.with_extension("c");
        let executable_path = root.with_extension("out");
        std::fs::write(&source_path, translation.as_str()).expect("write generated C");
        let compiler = Command::new("cc")
            .args([
                "-std=c11",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic",
            ])
            .arg(&source_path)
            .arg("-o")
            .arg(&executable_path)
            .output()
            .expect("C compiler runs");
        assert!(
            compiler.status.success(),
            "generated {name} C did not compile:\n{}\n{}",
            String::from_utf8_lossy(&compiler.stderr),
            translation.as_str()
        );
        let execution = Command::new(&executable_path)
            .output()
            .expect("generated overflow program runs");
        assert!(!execution.status.success(), "{name} overflow did not trap");
        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(executable_path);
    }
}

fn temporary_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pop-c-backend-{name}-{}", std::process::id()))
}
