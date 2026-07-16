use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn fixture_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pop-ffi-generate-{name}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ))
}

fn clean_root(root: &Path) {
    if root.exists() {
        std::fs::remove_dir_all(root).expect("remove prior generator fixture");
    }
}

fn descriptor(target: &str) -> String {
    format!(
        concat!(
            "@Ffi.Binding(\n",
            "    schemaVersion = 1,\n",
            "    platformTarget = \"{}\",\n",
            "    producerName = \"fixture-abi\",\n",
            "    producerVersion = \"1.0.0\",\n",
            "    outputNamespace = Native.Zlib.Unsafe,\n",
            ")\n",
            "namespace Native.Zlib.Binding\n",
            "\n",
            "@Ffi.C.Layout(size = 8, alignment = 4)\n",
            "internal record Pair\n",
            "    @Ffi.C.Offset(0)\n",
            "    left: Ffi.C.Int\n",
            "    @Ffi.C.Offset(4)\n",
            "    right: Ffi.C.Int\n",
            "end\n",
            "\n",
            "@Ffi.Foreign(\"compress_pair\", abi = \"C\")\n",
            "@Ffi.Binding.CallPolicy(nonblocking = true)\n",
            "@Ffi.Binding.ParameterPointer(parameter = destination, retention = Ffi.Binding.Retention.Call)\n",
            "@Ffi.Binding.ParameterPointer(parameter = source, retention = Ffi.Binding.Retention.Call)\n",
            "internal function compress(\n",
            "    destination: Ffi.Pointer<Pair>,\n",
            "    source: Ffi.ReadOnlyPointer<Pair>,\n",
            "    count: Ffi.C.Size,\n",
            "): Ffi.C.Int\n",
            "end\n",
        ),
        target
    )
}

fn callback_signature_fingerprint(target: &str, abi: &str) -> String {
    callback_layout_fingerprint(
        target,
        abi,
        &[
            "Ffi.C.Int(size=4,alignment=4)",
            "Ffi.CallbackContext(pointerWidth=64)",
        ],
        Some("Ffi.C.Int(size=4,alignment=4)"),
    )
}

fn callback_layout_fingerprint(
    target: &str,
    abi: &str,
    parameters: &[&str],
    result: Option<&str>,
) -> String {
    let mut descriptor = format!(
        "Pop.Ffi.CallbackSignature/1\nplatformTarget={target}\nabi={abi}\nparameterCount={}\n",
        parameters.len()
    );
    for (index, parameter) in parameters.iter().enumerate() {
        writeln!(descriptor, "parameter[{index}]={parameter}").expect("String write");
    }
    match result {
        Some(result) => {
            descriptor.push_str("resultCount=1\n");
            writeln!(descriptor, "result[0]={result}").expect("String write");
        }
        None => descriptor.push_str("resultCount=0\n"),
    }
    sha256(descriptor.as_bytes())
}

fn callback_descriptor(target: &str) -> String {
    format!(
        concat!(
            "@Ffi.Binding(\n",
            "    schemaVersion = 2,\n",
            "    platformTarget = \"{}\",\n",
            "    producerName = \"fixture-abi\",\n",
            "    producerVersion = \"1.0.0\",\n",
            "    outputNamespace = Native.Zlib.Unsafe,\n",
            ")\n",
            "namespace Native.Zlib.Binding\n",
            "\n",
            "@Ffi.Foreign(\"visit_values\", abi = \"C\")\n",
            "@Ffi.Binding.CallPolicy(nonblocking = false)\n",
            "@Ffi.Binding.CallbackPair(\n",
            "    callbackParameterIndex = 0,\n",
            "    contextParameterIndex = 1,\n",
            "    lifetime = Ffi.Binding.CallbackLifetime.CallScoped,\n",
            "    callbackAbi = Ffi.Binding.CallbackAbi.C,\n",
            "    signatureFingerprint = \"{}\",\n",
            "    thread = Ffi.Binding.CallbackThread.CallingThread,\n",
            "    concurrency = Ffi.Binding.CallbackConcurrency.Serialized,\n",
            "    reentrancy = Ffi.Binding.CallbackReentrancy.Forbidden,\n",
            "    panicPolicy = Ffi.Binding.CallbackPanic.AbortProcess,\n",
            ")\n",
            "internal function visitValues(\n",
            "    callback: Ffi.Function<function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int>,\n",
            "    context: Ffi.CallbackContext,\n",
            "): Ffi.C.Int\n",
            "end\n",
        ),
        target,
        callback_signature_fingerprint(target, "C")
    )
}

fn registered_callback_descriptor(target: &str) -> String {
    callback_descriptor(target)
        .replace(
            "Ffi.Binding.CallbackLifetime.CallScoped",
            "Ffi.Binding.CallbackLifetime.Registered",
        )
        .replace(
            "Ffi.Binding.CallbackThread.CallingThread",
            "Ffi.Binding.CallbackThread.AttachedThread",
        )
}

fn incompatible_callback_descriptor(target: &str) -> String {
    let c = callback_descriptor(target);
    let function = c
        .find("@Ffi.Foreign")
        .map(|start| &c[start..])
        .expect("callback function block");
    let system = function
        .replace("visit_values", "visit_values_system")
        .replace("visitValues", "visitValuesSystem")
        .replace(
            "Ffi.Binding.CallbackAbi.C",
            "Ffi.Binding.CallbackAbi.System",
        )
        .replace(
            &callback_signature_fingerprint(target, "C"),
            &callback_signature_fingerprint(target, "System"),
        );
    format!("{c}\n{system}")
}

fn record_pointer_callback_descriptor(target: &str) -> (String, String) {
    let record = "record(size=8,alignment=4,fields=[left@0:Ffi.C.Int(size=4,alignment=4);right@4:Ffi.C.Int(size=4,alignment=4)])";
    let pointer = format!("Ffi.ReadOnlyPointer<{record}>(size=8,alignment=8)");
    let fingerprint = callback_layout_fingerprint(
        target,
        "C",
        &[record, &pointer, "Ffi.CallbackContext(pointerWidth=64)"],
        Some(record),
    );
    let descriptor = format!(
        concat!(
            "@Ffi.Binding(\n",
            "    schemaVersion = 2,\n",
            "    platformTarget = \"{}\",\n",
            "    producerName = \"fixture-abi\",\n",
            "    producerVersion = \"1.0.0\",\n",
            "    outputNamespace = Native.Zlib.Unsafe,\n",
            ")\n",
            "namespace Native.Zlib.Binding\n",
            "\n",
            "@Ffi.C.Layout(size = 8, alignment = 4)\n",
            "internal record Pair\n",
            "    @Ffi.C.Offset(0)\n",
            "    left: Ffi.C.Int\n",
            "    @Ffi.C.Offset(4)\n",
            "    right: Ffi.C.Int\n",
            "end\n",
            "\n",
            "@Ffi.Foreign(\"visit_pairs\", abi = \"C\")\n",
            "@Ffi.Binding.CallPolicy(nonblocking = false)\n",
            "@Ffi.Binding.CallbackPair(\n",
            "    callbackParameterIndex = 0,\n",
            "    contextParameterIndex = 1,\n",
            "    lifetime = Ffi.Binding.CallbackLifetime.CallScoped,\n",
            "    callbackAbi = Ffi.Binding.CallbackAbi.C,\n",
            "    signatureFingerprint = \"{}\",\n",
            "    thread = Ffi.Binding.CallbackThread.CallingThread,\n",
            "    concurrency = Ffi.Binding.CallbackConcurrency.Serialized,\n",
            "    reentrancy = Ffi.Binding.CallbackReentrancy.Forbidden,\n",
            "    panicPolicy = Ffi.Binding.CallbackPanic.AbortProcess,\n",
            ")\n",
            "internal function visitPairs(\n",
            "    callback: Ffi.Function<function(pair: Pair, pointer: Ffi.ReadOnlyPointer<Pair>, context: Ffi.CallbackContext): Pair>,\n",
            "    context: Ffi.CallbackContext,\n",
            "): Ffi.C.Int\n",
            "end\n",
        ),
        target, fingerprint,
    );
    (descriptor, fingerprint)
}

fn write_fixture(name: &str, descriptor_text: &str, target: &str, linked: bool) -> PathBuf {
    let root = fixture_root(name);
    write_fixture_at(root, descriptor_text, target, linked)
}

fn write_fixture_at(root: PathBuf, descriptor_text: &str, target: &str, linked: bool) -> PathBuf {
    clean_root(&root);
    std::fs::create_dir_all(root.join("native")).expect("create descriptor directory");
    std::fs::write(root.join("native/zlib.popc"), descriptor_text).expect("write descriptor");
    let native = if linked {
        "[nativeLibraries]\nZlib = { kind = \"system\", name = \"z\" }\n"
    } else {
        ""
    };
    let native_library = if linked {
        "nativeLibrary = \"Zlib\", "
    } else {
        ""
    };
    std::fs::write(
        root.join("bubble.toml"),
        format!(
            "[package]\nname = \"Example.Bindings\"\nversion = \"0.1.0\"\nedition = \"2026\"\n{native}[platform.\"{target}\".ffiGenerators]\nZlib = {{ {native_library}descriptor = \"native/zlib.popc\", descriptorSha256 = \"{}\", outputDirectory = \"src/generated/zlib\" }}\n",
            sha256(descriptor_text.as_bytes())
        ),
    )
    .expect("write generator manifest");
    root
}

fn run_generate(root: &Path, target: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["ffi", "generate", "Zlib", "--manifestPath"])
        .arg(root.join("bubble.toml"))
        .args(["--platformTarget", target])
        .output()
        .expect("pop ffi generate runs")
}

fn run_package_check(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["check", "--manifestPath"])
        .arg(root.join("bubble.toml"))
        .output()
        .expect("pop check generated Package")
}

fn run_package_build(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pop"))
        .args(["build", "--manifestPath"])
        .arg(root.join("bubble.toml"))
        .output()
        .expect("pop build generated Package")
}

fn write_checked_fixture(name: &str) -> (PathBuf, &'static str) {
    write_checked_fixture_with_descriptor(name, &descriptor("x86_64-unknown-linux-gnu"))
}

fn write_checked_callback_fixture(name: &str) -> (PathBuf, &'static str) {
    write_checked_fixture_with_descriptor(name, &callback_descriptor("x86_64-unknown-linux-gnu"))
}

fn write_checked_registered_callback_fixture(name: &str) -> (PathBuf, &'static str) {
    write_checked_fixture_with_descriptor(
        name,
        &registered_callback_descriptor("x86_64-unknown-linux-gnu"),
    )
}

fn write_checked_fixture_with_descriptor(name: &str, input: &str) -> (PathBuf, &'static str) {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("driver crate is under repository root")
        .to_path_buf();
    let target = "x86_64-unknown-linux-gnu";
    let root = write_fixture_at(
        repository.join("target").join(format!(
            "pop-ffi-generated-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        )),
        input,
        target,
        true,
    );
    let manifest = std::fs::read_to_string(root.join("bubble.toml")).expect("manifest");
    std::fs::write(
        root.join("bubble.toml"),
        manifest.replace(
            "[nativeLibraries]",
            "[dependencies]\nPopFfi = { path = \"../../crates/extensions/ffi\", version = \"0.1.0\", bubble = \"Pop.Ffi\" }\n[nativeLibraries]",
        ),
    )
    .expect("add exact Pop.Ffi dependency");
    std::fs::create_dir_all(root.join("src")).expect("create source root");
    std::fs::write(
        root.join("src/lib.pop"),
        "namespace Example.Bindings\n\
         public function bindingMarker(): Int\n\
             return 1\n\
         end\n",
    )
    .expect("write library root");
    let generate = run_generate(&root, target);
    assert!(
        generate.status.success(),
        "generation failed: {}",
        String::from_utf8_lossy(&generate.stderr)
    );
    (root, target)
}

#[test]
fn adr_0094_generates_canonical_typed_callback_pair_source_and_metadata() {
    let target = "x86_64-unknown-linux-gnu";
    let input = callback_descriptor(target);
    let root = write_fixture("callback-pair", &input, target, true);

    let generated = run_generate(&root, target);
    assert!(
        generated.status.success(),
        "callback generation failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let output = root.join("src/generated/zlib");
    let source = std::fs::read_to_string(output.join("bindings.pop")).expect("source");
    assert_eq!(
        source,
        "@Ffi.Link(\"Zlib\")\nnamespace Native.Zlib.Unsafe\n\n@Ffi.Foreign(\"visit_values\")\ninternal function visitValues(callback: Ffi.Function<function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int>, context: Ffi.CallbackContext): Ffi.C.Int\nend\n"
    );
    assert!(!source.contains("Ffi.Binding.CallbackPair"));
    assert!(!source.contains("Ffi.Nonblocking"));

    let metadata = std::fs::read_to_string(output.join("native-bindings.popc")).expect("metadata");
    assert!(metadata.starts_with("@Ffi.GeneratedBindings(\n    schemaVersion = 2,"));
    assert!(metadata.contains("    parserVersion = 2,"));
    assert!(metadata.contains("@Ffi.Binding.CallbackPair(\n"));
    assert!(metadata.contains("    callbackParameterIndex = 0,"));
    assert!(metadata.contains("    contextParameterIndex = 1,"));
    assert!(metadata.contains(&format!(
        "    signatureFingerprint = \"{}\",",
        callback_signature_fingerprint(target, "C")
    )));
    assert!(metadata.contains("    lifetime = Ffi.Binding.CallbackLifetime.CallScoped,"));
    assert!(metadata.contains("    callbackAbi = Ffi.Binding.CallbackAbi.C,"));
    assert!(metadata.contains("    thread = Ffi.Binding.CallbackThread.CallingThread,"));
    assert!(metadata.contains("    concurrency = Ffi.Binding.CallbackConcurrency.Serialized,"));
    assert!(metadata.contains("    reentrancy = Ffi.Binding.CallbackReentrancy.Forbidden,"));
    assert!(metadata.contains("    panicPolicy = Ffi.Binding.CallbackPanic.AbortProcess,"));
    assert!(!metadata.to_ascii_lowercase().contains("json"));

    clean_root(&root);
}

#[test]
fn adr_0094_fingerprints_expanded_record_and_pointer_callback_layouts() {
    let target = "x86_64-unknown-linux-gnu";
    let (input, fingerprint) = record_pointer_callback_descriptor(target);
    let root = write_fixture("callback-record-pointer", &input, target, true);
    let generated = run_generate(&root, target);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let metadata = std::fs::read_to_string(root.join("src/generated/zlib/native-bindings.popc"))
        .expect("generated metadata");
    assert!(metadata.contains(&format!("signatureFingerprint = \"{fingerprint}\"")));
    assert!(metadata.contains(
        "Ffi.Function<function(pair: Pair, pointer: Ffi.ReadOnlyPointer<Pair>, context: Ffi.CallbackContext): Pair>"
    ));
    clean_root(&root);
}

#[test]
fn adr_0094_preserves_schema_1_callback_rejection_and_fails_closed_on_policy_mismatch() {
    let target = "x86_64-unknown-linux-gnu";
    let schema_1 = callback_descriptor(target).replace("schemaVersion = 2", "schemaVersion = 1");
    let root = write_fixture("schema-1-callback", &schema_1, target, true);
    let rejected = run_generate(&root, target);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5085"));
    clean_root(&root);

    let cases = [
        (
            "callback-fingerprint",
            callback_descriptor(target).replace(
                &callback_signature_fingerprint(target, "C"),
                &"0".repeat(64),
            ),
        ),
        (
            "callback-nonblocking",
            callback_descriptor(target).replace("nonblocking = false", "nonblocking = true"),
        ),
        (
            "callback-index",
            callback_descriptor(target)
                .replace("callbackParameterIndex = 0", "callbackParameterIndex = 1"),
        ),
        (
            "callback-context-index",
            callback_descriptor(target)
                .replace("contextParameterIndex = 1", "contextParameterIndex = 2"),
        ),
        (
            "callback-thread",
            callback_descriptor(target).replace(
                "Ffi.Binding.CallbackThread.CallingThread",
                "Ffi.Binding.CallbackThread.AttachedThread",
            ),
        ),
        (
            "callback-abi",
            callback_descriptor(target).replace(
                "Ffi.Binding.CallbackAbi.C",
                "Ffi.Binding.CallbackAbi.CUnwind",
            ),
        ),
    ];
    for (name, input) in cases {
        let root = write_fixture(name, &input, target, true);
        let rejected = run_generate(&root, target);
        assert!(!rejected.status.success(), "{name}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("POP5084")
                || String::from_utf8_lossy(&rejected.stderr).contains("POP5085"),
            "{name}: {}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(!root.join("src/generated/zlib").exists());
        clean_root(&root);
    }
}

#[test]
fn adr_0094_package_preflight_attaches_callback_metadata_before_hir() {
    let (root, _) = write_checked_callback_fixture("callback-check");
    std::fs::write(
        root.join("src/lib.pop"),
        "namespace Example.Bindings\n\
         private type CallbackSignature = function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int\n\
         public function invoke(): Ffi.C.Int\n\
             return Ffi.withCallback(\n\
                 function(value: Ffi.C.Int, _: Ffi.CallbackContext): Ffi.C.Int\n\
                     return value\n\
                 end,\n\
                 function(callbackFunction: Ffi.Function<CallbackSignature>, context: Ffi.CallbackContext): Ffi.C.Int\n\
                     return Native.Zlib.Unsafe.visitValues(callbackFunction, context)\n\
                 end\n\
             )\n\
         end\n",
    )
    .expect("write exact attached callback use");
    let check = run_package_check(&root);
    assert!(
        check.status.success(),
        "generated callback bindings did not type-check with their selected metadata: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let source = root.join("src/generated/zlib/bindings.pop");
    let source_text = std::fs::read_to_string(&source).expect("generated source");
    std::fs::write(
        &source,
        source_text.replace("context: Ffi.CallbackContext", "context: Ffi.C.Size"),
    )
    .expect("tamper resolved callback signature");
    let rejected = run_package_check(&root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5086"));

    clean_root(&root);
}

#[test]
fn adr_0094_rejects_an_unused_scoped_callback_pair_before_hir() {
    let (root, _) = write_checked_callback_fixture("callback-unused-pair");
    std::fs::write(
        root.join("src/lib.pop"),
        "namespace Example.Bindings\n\
         private type CallbackSignature = function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int\n\
         public function run(): Int\n\
             return Ffi.withCallback(\n\
                 function(value: Ffi.C.Int, _: Ffi.CallbackContext): Ffi.C.Int\n\
                     return value\n\
                 end,\n\
                 function(callbackFunction: Ffi.Function<CallbackSignature>, callbackContext: Ffi.CallbackContext): Int\n\
                     return 0\n\
                 end\n\
             )\n\
         end\n",
    )
    .expect("write unused pair source");

    let check = run_package_check(&root);
    assert!(!check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("POP2003"),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    clean_root(&root);
}

#[test]
fn adr_0094_rejects_incompatible_pair_contracts_before_hir() {
    let (root, _) = write_checked_fixture_with_descriptor(
        "callback-incompatible-pairs",
        &incompatible_callback_descriptor("x86_64-unknown-linux-gnu"),
    );
    std::fs::write(
        root.join("src/lib.pop"),
        "namespace Example.Bindings\n\
         private type CallbackSignature = function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int\n\
         public function run(): Ffi.C.Int\n\
             return Ffi.withCallback(\n\
                 function(value: Ffi.C.Int, _: Ffi.CallbackContext): Ffi.C.Int\n\
                     return value\n\
                 end,\n\
                 function(callbackFunction: Ffi.Function<CallbackSignature>, context: Ffi.CallbackContext): Ffi.C.Int\n\
                     Native.Zlib.Unsafe.visitValues(callbackFunction, context)\n\
                     return Native.Zlib.Unsafe.visitValuesSystem(callbackFunction, context)\n\
                 end\n\
             )\n\
         end\n",
    )
    .expect("write incompatible pair source");

    let check = run_package_check(&root);
    assert!(!check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("POP2003"),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    clean_root(&root);
}

#[test]
fn adr_0094_selects_registered_callback_contract_at_pair_time() {
    let (root, _) = write_checked_registered_callback_fixture("callback-registered-pair");
    std::fs::write(
        root.join("src/lib.pop"),
        "namespace Example.Bindings\n\
         private type CallbackSignature = function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int\n\
         public function openCallback(): Result<Ffi.RegisteredCallback<CallbackSignature>, Ffi.CallbackOpenError>\n\
             return Ffi.Callback.open(\n\
                 function(value: Ffi.C.Int, _: Ffi.CallbackContext): Ffi.C.Int\n\
                     return value\n\
                 end,\n\
                 Ffi.CallbackThread.AttachedThread\n\
             )\n\
         end\n\
         public function invoke(callback: Ffi.RegisteredCallback<CallbackSignature>): Result<Ffi.C.Int, Ffi.CallbackClosedError>\n\
             return Ffi.Callback.withPair(\n\
                 callback,\n\
                 function(callbackFunction: Ffi.Function<CallbackSignature>, context: Ffi.CallbackContext): Ffi.C.Int\n\
                     return Native.Zlib.Unsafe.visitValues(callbackFunction, context)\n\
                 end\n\
             )\n\
         end\n",
    )
    .expect("write registered pair source");

    let check = run_package_check(&root);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    clean_root(&root);
}

#[test]
fn adr_0093_generates_deterministic_typed_popc_bindings_without_process_input() {
    let target = "x86_64-unknown-linux-gnu";
    let input = descriptor(target);
    let root = write_fixture("deterministic", &input, target, true);

    let first = run_generate(&root, target);
    assert!(
        first.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stdout.is_empty());
    assert!(first.stderr.is_empty());
    let output = root.join("src/generated/zlib");
    let source = std::fs::read(output.join("bindings.pop")).expect("generated Pop source");
    let shim = std::fs::read(output.join("bindings.c")).expect("generated shim unit");
    let metadata =
        std::fs::read(output.join("native-bindings.popc")).expect("generated typed metadata");

    assert_eq!(
        String::from_utf8(source.clone()).expect("UTF-8 source"),
        "@Ffi.Link(\"Zlib\")\nnamespace Native.Zlib.Unsafe\n\n@Ffi.C.Layout\ninternal record Pair\n    left: Ffi.C.Int\n    right: Ffi.C.Int\nend\n\n@Ffi.Foreign(\"compress_pair\")\n@Ffi.Nonblocking\ninternal function compress(destination: Ffi.Pointer<Pair>, source: Ffi.ReadOnlyPointer<Pair>, count: Ffi.C.Size): Ffi.C.Int\nend\n"
    );
    assert_eq!(
        String::from_utf8(shim.clone()).expect("UTF-8 shim"),
        "/* Generated by pop ffi generate; schema 1 has no C shims. */\n"
    );
    let metadata_text = String::from_utf8(metadata.clone()).expect("UTF-8 metadata");
    assert!(metadata_text.starts_with("@Ffi.GeneratedBindings(\n"));
    assert!(metadata_text.contains("descriptorPath = \"native/zlib.popc\","));
    assert!(metadata_text.contains(&format!(
        "descriptorSha256 = \"{}\",",
        sha256(input.as_bytes())
    )));
    assert!(metadata_text.contains(&format!("sourceSha256 = \"{}\",", sha256(&source))));
    assert!(metadata_text.contains(&format!("shimSha256 = \"{}\",", sha256(&shim))));
    assert!(metadata_text.contains("outputNamespace = Native.Zlib.Unsafe,"));
    assert!(!metadata_text.contains(&root.to_string_lossy().to_string()));
    assert!(!metadata_text.to_ascii_lowercase().contains("json"));

    let second = run_generate(&root, target);
    assert!(
        second.status.success(),
        "byte-identical generation is a no-op: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        std::fs::read(output.join("bindings.pop")).expect("stable source"),
        source
    );
    assert_eq!(
        std::fs::read(output.join("bindings.c")).expect("stable shim"),
        shim
    );
    assert_eq!(
        std::fs::read(output.join("native-bindings.popc")).expect("stable metadata"),
        metadata
    );

    clean_root(&root);
}

#[test]
fn adr_0093_default_c_binding_omits_link_attribute() {
    let target = "x86_64-unknown-linux-gnu";
    let input = descriptor(target);
    let root = write_fixture("default-c", &input, target, false);

    let result = run_generate(&root, target);
    assert!(
        result.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let source = std::fs::read_to_string(root.join("src/generated/zlib/bindings.pop"))
        .expect("generated source");
    assert!(source.starts_with("namespace Native.Zlib.Unsafe\n"));
    assert!(!source.contains("Ffi.Link"));

    clean_root(&root);
}

#[test]
fn adr_0093_generated_pop_source_parses_and_type_checks_with_pop_ffi() {
    let (root, _) = write_checked_fixture("check");
    let check = run_package_check(&root);
    assert!(
        check.status.success(),
        "generated bindings did not type-check: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    clean_root(&root);
}

#[test]
fn adr_0093_package_preflight_rejects_tampered_generated_source_and_metadata_hash() {
    let (source_root, _) = write_checked_fixture("tampered-source");
    let source = source_root.join("src/generated/zlib/bindings.pop");
    let source_text = std::fs::read_to_string(&source).expect("source");
    std::fs::write(&source, format!("{source_text}\n")).expect("tamper generated source");
    let rejected = run_package_check(&source_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5086"));
    clean_root(&source_root);

    let (metadata_root, _) = write_checked_fixture("tampered-metadata");
    let metadata = metadata_root.join("src/generated/zlib/native-bindings.popc");
    let text = std::fs::read_to_string(&metadata).expect("metadata");
    let source_hash = text
        .lines()
        .find(|line| line.trim_start().starts_with("sourceSha256 ="))
        .and_then(|line| line.split('"').nth(1))
        .expect("source hash");
    std::fs::write(&metadata, text.replace(source_hash, &"0".repeat(64)))
        .expect("tamper metadata inventory hash");
    let rejected = run_package_check(&metadata_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5086"));
    clean_root(&metadata_root);
}

#[test]
fn adr_0093_package_preflight_rejects_missing_and_extra_generated_files() {
    let (missing_root, _) = write_checked_fixture("missing-output");
    std::fs::remove_file(missing_root.join("src/generated/zlib/bindings.c"))
        .expect("remove generated shim");
    let rejected = run_package_build(&missing_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5086"));
    clean_root(&missing_root);

    let (directory_root, _) = write_checked_fixture("missing-directory");
    std::fs::remove_dir_all(directory_root.join("src/generated/zlib"))
        .expect("remove generated directory");
    let rejected = run_package_check(&directory_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5086"));
    clean_root(&directory_root);

    let (extra_root, _) = write_checked_fixture("extra-output");
    std::fs::write(
        extra_root.join("src/generated/zlib/untracked.pop"),
        "namespace Injected\n",
    )
    .expect("write unexpected generated file");
    let rejected = run_package_check(&extra_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5086"));
    clean_root(&extra_root);
}

#[cfg(unix)]
#[test]
fn adr_0093_package_preflight_rejects_symlinked_generated_files() {
    use std::os::unix::fs::symlink;

    let (root, _) = write_checked_fixture("symlinked-output");
    let source = root.join("src/generated/zlib/bindings.pop");
    let actual = root.join("native/generated-source.pop");
    std::fs::rename(&source, &actual).expect("move generated source outside generated output");
    symlink("../../../native/generated-source.pop", &source).expect("symlink generated source");
    let rejected = run_package_check(&root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5086"));
    clean_root(&root);

    let (parent_root, _) = write_checked_fixture("symlinked-parent");
    std::fs::rename(
        parent_root.join("src/generated"),
        parent_root.join("native/generated-root"),
    )
    .expect("move generated parent outside the source tree");
    symlink(
        "../native/generated-root",
        parent_root.join("src/generated"),
    )
    .expect("symlink generated parent");
    let rejected = run_package_check(&parent_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5081"));
    clean_root(&parent_root);
}

#[test]
fn adr_0093_package_preflight_rejects_metadata_target_and_descriptor_hash_mismatch() {
    let (target_root, _) = write_checked_fixture("metadata-target");
    let metadata = target_root.join("src/generated/zlib/native-bindings.popc");
    let text = std::fs::read_to_string(&metadata).expect("metadata");
    std::fs::write(
        &metadata,
        text.replace(
            "platformTarget = \"x86_64-unknown-linux-gnu\"",
            "platformTarget = \"bpfel-unknown-none\"",
        ),
    )
    .expect("change metadata target");
    let rejected = run_package_check(&target_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5084"));
    clean_root(&target_root);

    let (hash_root, _) = write_checked_fixture("descriptor-hash");
    let manifest = hash_root.join("bubble.toml");
    let text = std::fs::read_to_string(&manifest).expect("manifest");
    let descriptor_hash = sha256(descriptor("x86_64-unknown-linux-gnu").as_bytes());
    std::fs::write(&manifest, text.replace(&descriptor_hash, &"0".repeat(64)))
        .expect("change manifest descriptor hash");
    let rejected = run_package_check(&hash_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("POP5081"));
    clean_root(&hash_root);
}

#[test]
fn adr_0093_rejects_hash_target_noncanonical_and_source_injection_before_publish() {
    let target = "x86_64-unknown-linux-gnu";
    let cases = [
        (
            "target-mismatch",
            descriptor("aarch64-unknown-linux-gnu"),
            "POP5084",
        ),
        (
            "noncanonical",
            descriptor(target).replace("    schemaVersion", "  schemaVersion"),
            "POP5082",
        ),
        (
            "source-injection",
            descriptor(target).replace(
                "end\n",
                "    return Ffi.Unsafe.pointerFromAddress<<Byte>>(0)\nend\n",
            ),
            "POP5082",
        ),
        (
            "reserved-identifier",
            descriptor(target).replace("function compress(", "function end("),
            "POP5082",
        ),
        (
            "layout-mismatch",
            descriptor(target).replace(
                "@Ffi.C.Layout(size = 8, alignment = 4)",
                "@Ffi.C.Layout(size = 12, alignment = 4)",
            ),
            "POP5084",
        ),
    ];

    for (name, input, code) in cases {
        let root = write_fixture(name, &input, target, true);
        let result = run_generate(&root, target);
        assert!(!result.status.success(), "{name}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(code),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!root.join("src/generated/zlib").exists(), "{name}");
        clean_root(&root);
    }

    let input = descriptor(target);
    let root = write_fixture("hash", &input, target, true);
    std::fs::write(root.join("native/zlib.popc"), format!("{input}\n"))
        .expect("mutate hashed descriptor");
    let result = run_generate(&root, target);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("POP5081"));
    assert!(!root.join("src/generated/zlib").exists());
    clean_root(&root);
}

#[test]
fn adr_0093_rejects_manifest_traversal_before_reading_an_input() {
    let target = "x86_64-unknown-linux-gnu";
    let input = descriptor(target);
    let root = write_fixture("traversal", &input, target, true);
    let manifest = std::fs::read_to_string(root.join("bubble.toml")).expect("manifest");
    std::fs::write(
        root.join("bubble.toml"),
        manifest.replace("native/zlib.popc", "../zlib.popc"),
    )
    .expect("write traversal manifest");

    let result = run_generate(&root, target);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("POP5080"));
    assert!(!root.join("src/generated/zlib").exists());
    clean_root(&root);
}

#[cfg(unix)]
#[test]
fn adr_0093_rejects_symlinked_descriptor_inputs() {
    use std::os::unix::fs::symlink;

    let target = "x86_64-unknown-linux-gnu";
    let input = descriptor(target);
    let root = write_fixture("symlink", &input, target, true);
    std::fs::rename(
        root.join("native/zlib.popc"),
        root.join("native/actual.popc"),
    )
    .expect("move descriptor");
    symlink("actual.popc", root.join("native/zlib.popc")).expect("symlink descriptor");

    let result = run_generate(&root, target);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("POP5081"));
    assert!(!root.join("src/generated/zlib").exists());
    clean_root(&root);
}

#[test]
fn adr_0093_output_conflict_preserves_existing_files_and_cleans_staging() {
    let target = "x86_64-unknown-linux-gnu";
    let input = descriptor(target);
    let root = write_fixture("conflict", &input, target, true);
    let output = root.join("src/generated/zlib");
    std::fs::create_dir_all(&output).expect("create conflicting output");
    std::fs::write(output.join("local.pop"), "local edits\n").expect("write conflict");

    let result = run_generate(&root, target);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("POP5086"));
    assert_eq!(
        std::fs::read_to_string(output.join("local.pop")).expect("preserved local file"),
        "local edits\n"
    );
    assert!(
        std::fs::read_dir(root.join("src/generated"))
            .expect("generated parent")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp"))
    );

    clean_root(&root);
}

#[test]
fn ffi_generate_rejects_raw_header_tool_flag_and_output_overrides() {
    for forbidden in ["--header", "--tool", "--flags", "--output"] {
        let output = Command::new(env!("CARGO_BIN_EXE_pop"))
            .args([
                "ffi",
                "generate",
                "Zlib",
                "--manifestPath",
                "bubble.toml",
                "--platformTarget",
                "x86_64-unknown-linux-gnu",
                forbidden,
                "injected",
            ])
            .output()
            .expect("pop command runs");
        assert!(!output.status.success(), "{forbidden}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unsupported option"),
            "{forbidden}: {stderr}"
        );
    }
}
