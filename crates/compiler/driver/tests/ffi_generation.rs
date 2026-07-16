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
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("driver crate is under repository root")
        .to_path_buf();
    let target = "x86_64-unknown-linux-gnu";
    let input = descriptor(target);
    let root = write_fixture_at(
        repository.join("target").join(format!(
            "pop-ffi-generated-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        )),
        &input,
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
