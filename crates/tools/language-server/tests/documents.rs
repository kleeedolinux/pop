use pop_driver::{artifact_sha256_hex, generate_ffi_bindings};
use pop_language_server::{
    DocumentUri, DocumentVersion, LanguageServer, LanguageServerError, ProtocolPosition,
};
use pop_query::CancellationToken;

#[test]
fn open_change_and_close_preserve_identity_and_require_newer_versions() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/main.pop").expect("URI");
    let first = server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            "namespace Example\npublic function value(): Int\n    return missing\nend\n",
            &CancellationToken::new(),
        )
        .expect("open document");
    assert_eq!(first.version(), DocumentVersion::new(1));
    assert!(
        first
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "POP1002")
    );

    let stale = server
        .change(
            &uri,
            DocumentVersion::new(1),
            "namespace Example\n",
            &CancellationToken::new(),
        )
        .expect_err("same version is stale");
    assert!(matches!(stale, LanguageServerError::StaleVersion { .. }));

    let changed = server
        .change(
            &uri,
            DocumentVersion::new(2),
            "namespace Example\npublic function broken(\n",
            &CancellationToken::new(),
        )
        .expect("new version");
    assert_eq!(
        changed.file(),
        first.file(),
        "document identity remains stable"
    );
    assert_eq!(changed.version(), DocumentVersion::new(2));
    assert!(!changed.diagnostics().is_empty());
    assert!(
        changed
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.code().starts_with("POP"))
    );

    assert!(server.close(&uri));
    assert!(!server.close(&uri));
    assert!(matches!(
        server.analyze(&uri, &CancellationToken::new()),
        Err(LanguageServerError::DocumentNotOpen { .. })
    ));
}

#[test]
fn misspelled_return_publishes_the_root_cause_span_and_safe_correction() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/nativeClass.pop").expect("URI");
    let analysis = server
        .open(
            uri,
            DocumentVersion::new(1),
            "namespace Example\nprivate record Box\n    value: Int\nend\nprivate function make(value: Int): Box\n    retun Box { value = value }\nend\n",
            &CancellationToken::new(),
        )
        .expect("analysis");
    let diagnostic = analysis
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == "POP0002")
        .expect("syntax diagnostic");

    assert_eq!(diagnostic.range().start(), ProtocolPosition::new(5, 4));
    assert_eq!(diagnostic.range().end(), ProtocolPosition::new(5, 9));
    assert!(
        diagnostic.message().contains("expected `return`"),
        "{}",
        diagnostic.message()
    );
    assert!(
        diagnostic.message().contains("found `retun`"),
        "{}",
        diagnostic.message()
    );
    let fix = diagnostic.fixes().first().expect("safe keyword correction");
    assert!(fix.is_safe());
    assert_eq!(fix.id(), "replaceMisspelledReturn");
    assert_eq!(fix.equivalence_key(), Some("replaceMisspelledReturn"));
    assert_eq!(fix.edits().len(), 1);
    assert_eq!(fix.edits()[0].range(), diagnostic.range());
    assert_eq!(fix.edits()[0].replacement(), "return");
}

#[test]
fn repeated_ignored_parameters_do_not_publish_duplicate_binding_diagnostics() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/ignoredParameters.pop").expect("URI");
    let analysis = server
        .open(
            uri,
            DocumentVersion::new(1),
            "namespace Example\n\
             function calculate(): Int\n\
                 local discard = function(_: Int, _: String): Int\n\
                     return 42\n\
                 end\n\
                 return discard(1, \"ignored\")\n\
             end\n",
            &CancellationToken::new(),
        )
        .expect("analysis");

    assert!(
        analysis.diagnostics().is_empty(),
        "ignored parameters never create lexical or duplicate bindings: {:?}",
        analysis.diagnostics()
    );
}

#[test]
fn standalone_analysis_receives_the_reserved_standard_reference() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/standard.pop").expect("URI");
    let analysis = server
        .open(
            uri,
            DocumentVersion::new(1),
            "namespace Example\nusing Math = Pop.Math\nfunction minimum(left: Int, right: Int): Int\n    return Math.min(left, right)\nend\n",
            &CancellationToken::new(),
        )
        .expect("analysis");

    assert!(
        analysis.diagnostics().is_empty(),
        "Pop.Standard is an implicit verified reference for every normal Bubble: {:?}",
        analysis.diagnostics()
    );
}

#[test]
fn hover_uses_checked_compiler_documentation_and_exact_source_signature() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/math.pop").expect("URI");
    server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            "namespace Example\n\
             --- <summary>\n\
             --- Adds two integers.\n\
             --- </summary>\n\
             public function add(left: Int, right: Int): Int\n\
                 return left + right\n\
             end\n",
            &CancellationToken::new(),
        )
        .expect("open documented function");

    let hover = server
        .hover(
            &uri,
            ProtocolPosition::new(4, 17),
            &CancellationToken::new(),
        )
        .expect("hover query")
        .expect("function hover");
    assert_eq!(
        hover.signature(),
        "public function add(left: Int, right: Int): Int"
    );
    assert_eq!(hover.summary(), Some("Adds two integers."));
    assert_eq!(hover.range().start(), ProtocolPosition::new(4, 16));

    assert!(
        server
            .hover(&uri, ProtocolPosition::new(0, 0), &CancellationToken::new(),)
            .expect("empty hover")
            .is_none()
    );
}

#[test]
fn document_symbols_are_compiler_indexed_and_utf16_positioned() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/symbols.pop").expect("URI");
    server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            "namespace Example\npublic record User\n    name: String\nend\n\nfunction load(): Int\n    return 1\nend\n",
            &CancellationToken::new(),
        )
        .expect("open symbols");

    let symbols = server
        .document_symbols(&uri, &CancellationToken::new())
        .expect("document symbols");
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0].name(), "User");
    assert_eq!(symbols[0].kind(), "record");
    assert_eq!(symbols[0].selection_range().end().character(), 18);
    assert_eq!(symbols[1].name(), "load");
    assert_eq!(symbols[1].kind(), "function");
}

#[test]
fn malformed_documentation_is_diagnosed_and_never_enters_hover() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/broken-doc.pop").expect("URI");
    let analysis = server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            "namespace Example\n--- <summary>Broken\npublic function value(): Int\n    return 1\nend\n",
            &CancellationToken::new(),
        )
        .expect("open malformed documentation");
    let diagnostic = analysis
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == "POP6401")
        .expect("malformed documentation warning");
    assert_eq!(diagnostic.warning_wave(), Some(1));
    assert_eq!(diagnostic.warning_group(), Some("Documentation"));
    assert_eq!(diagnostic.suppression_key(), Some("POP6401"));
    let hover = server
        .hover(
            &uri,
            ProtocolPosition::new(2, 17),
            &CancellationToken::new(),
        )
        .expect("hover query")
        .expect("declaration signature remains available");
    assert_eq!(hover.summary(), None);

    assert!(server.close(&uri));
    assert!(matches!(
        server.hover(
            &uri,
            ProtocolPosition::new(2, 17),
            &CancellationToken::new(),
        ),
        Err(LanguageServerError::DocumentNotOpen { .. })
    ));
}

#[test]
fn hover_preserves_a_multiline_function_signature() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/multiline.pop").expect("URI");
    server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            "namespace Example\npublic function add(\n    left: Int,\n    right: Int\n): Int\n    return left + right\nend\n",
            &CancellationToken::new(),
        )
        .expect("open multiline signature");

    let hover = server
        .hover(
            &uri,
            ProtocolPosition::new(1, 17),
            &CancellationToken::new(),
        )
        .expect("hover query")
        .expect("function hover");
    assert_eq!(
        hover.signature(),
        "public function add(\n    left: Int,\n    right: Int\n): Int"
    );
}

#[test]
fn analysis_honors_cancellation_without_publishing_partial_results() {
    let mut server = LanguageServer::initialize(Some("pt-BR")).expect("server");
    let uri = DocumentUri::new("file:///workspace/cancel.pop").expect("URI");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = server
        .open(
            uri,
            DocumentVersion::new(1),
            "namespace Example\n",
            &cancellation,
        )
        .expect_err("cancelled open");
    assert_eq!(error, LanguageServerError::Cancelled);
    assert_eq!(server.document_count(), 0);
}

#[test]
fn duplicate_open_and_cancelled_change_preserve_the_published_snapshot() {
    let mut server = LanguageServer::initialize(Some("es")).expect("server");
    let uri = DocumentUri::new("file:///workspace/stable.pop").expect("URI");
    let opened = server
        .open(
            uri.clone(),
            DocumentVersion::new(4),
            "namespace Example\n",
            &CancellationToken::new(),
        )
        .expect("open document");

    let duplicate = server
        .open(
            uri.clone(),
            DocumentVersion::new(5),
            "namespace Replacement\n",
            &CancellationToken::new(),
        )
        .expect_err("duplicate open");
    assert!(matches!(
        duplicate,
        LanguageServerError::DocumentAlreadyOpen { .. }
    ));

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = server
        .change(
            &uri,
            DocumentVersion::new(5),
            "namespace Example\n§\n",
            &cancellation,
        )
        .expect_err("cancelled change");
    assert_eq!(cancelled, LanguageServerError::Cancelled);

    let current = server
        .analyze(&uri, &CancellationToken::new())
        .expect("published snapshot");
    assert_eq!(current.file(), opened.file());
    assert_eq!(current.version(), DocumentVersion::new(4));
    assert!(current.diagnostics().is_empty());
}

#[test]
fn protocol_positions_use_utf16_code_units() {
    let mut server = LanguageServer::initialize(Some("ja")).expect("server");
    let uri = DocumentUri::new("file:///workspace/unicode.pop").expect("URI");
    server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            "namespace Example\n\"😀\" §\n",
            &CancellationToken::new(),
        )
        .expect("open Unicode document");
    let analysis = server
        .analyze(&uri, &CancellationToken::new())
        .expect("analysis");
    let diagnostic = analysis
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == "POP0001")
        .expect("invalid character diagnostic");
    assert_eq!(diagnostic.range().start(), ProtocolPosition::new(1, 5));
}

#[test]
fn server_errors_render_with_the_session_catalog() {
    let server = LanguageServer::initialize(Some("zh-Hans")).expect("server");
    let uri = DocumentUri::new("file:///workspace/missing.pop").expect("URI");
    let error = server
        .analyze(&uri, &CancellationToken::new())
        .expect_err("missing document");
    let rendered = server.render_error(&error).expect("localized server error");
    assert!(rendered.contains("未打开"));
    assert!(rendered.contains(uri.as_str()));
}

#[test]
fn dependency_free_package_modules_are_analyzed_as_one_bubble() {
    let root = std::env::temp_dir().join(format!("PopLspProject{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("bubble.toml"),
        "[package]\nname = \"Studio.Project\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let active = "namespace Studio.Project\nfunction value(): Int\n    return helper()\nend\n";
    std::fs::write(root.join("src/lib.pop"), active).unwrap();
    std::fs::write(
        root.join("src/helper.pop"),
        "namespace Studio.Project\nfunction helper(): Int\n    return 42\nend\n",
    )
    .unwrap();
    let uri = DocumentUri::new(format!("file://{}", root.join("src/lib.pop").display())).unwrap();
    let mut server = LanguageServer::initialize(Some("en")).unwrap();
    let analysis = server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            active,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        analysis.diagnostics().is_empty(),
        "same-Bubble helper must resolve: {:?}",
        analysis.diagnostics()
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn callback_descriptor(platform_target: &str) -> String {
    let callback_layout = format!(
        concat!(
            "Pop.Ffi.CallbackSignature/1\n",
            "platformTarget={}\n",
            "abi=C\n",
            "parameterCount=2\n",
            "parameter[0]=Ffi.C.Int(size=4,alignment=4)\n",
            "parameter[1]=Ffi.CallbackContext(pointerWidth=64)\n",
            "resultCount=1\n",
            "result[0]=Ffi.C.Int(size=4,alignment=4)\n",
        ),
        platform_target
    );
    let callback_fingerprint = artifact_sha256_hex(callback_layout.as_bytes());
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
        platform_target, callback_fingerprint,
    )
}

fn write_generated_callback_fixture(root: &std::path::Path, platform_target: &str) -> String {
    std::fs::create_dir_all(root.join("native")).unwrap();
    std::fs::create_dir_all(root.join("dependencies/ffi/src")).unwrap();
    std::fs::write(
        root.join("dependencies/ffi/bubble.toml"),
        "[package]\nname = \"Pop.Ffi\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    std::fs::write(root.join("dependencies/ffi/src/lib.pop"), "namespace Ffi\n").unwrap();
    let descriptor = callback_descriptor(platform_target);
    std::fs::write(root.join("native/example.popc"), &descriptor).unwrap();
    let descriptor_sha256 = artifact_sha256_hex(descriptor.as_bytes());
    std::fs::write(
        root.join("bubble.toml"),
        format!(
            "[package]\nname = \"Studio.Ffi\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
             [dependencies]\n\
             PopFfi = {{ path = \"dependencies/ffi\", version = \"0.1.0\", bubble = \"Pop.Ffi\" }}\n\
             [nativeLibraries]\n\
             Zlib = {{ kind = \"system\", name = \"z\" }}\n\
             [platform.\"{platform_target}\".ffiGenerators]\n\
             Example = {{ nativeLibrary = \"Zlib\", descriptor = \"native/example.popc\", descriptorSha256 = \"{descriptor_sha256}\", outputDirectory = \"src/generated/example\" }}\n"
        ),
    )
    .unwrap();
    generate_ffi_bindings(&root.join("bubble.toml"), platform_target, "Example")
        .expect("canonical callback bindings");
    std::fs::read_to_string(root.join("src/generated/example/bindings.pop")).unwrap()
}

#[test]
fn direct_pop_ffi_dependency_attaches_verified_generated_callback_metadata() {
    let root = std::env::temp_dir().join(format!("PopLspGeneratedFfi{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let platform_target = "x86_64-unknown-linux-gnu";
    let active = write_generated_callback_fixture(&root, platform_target);
    let uri = DocumentUri::new(format!(
        "file://{}",
        root.join("src/generated/example/bindings.pop").display()
    ))
    .unwrap();
    let mut server = LanguageServer::initialize(Some("en")).unwrap();
    let analysis = server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            active.clone(),
            &CancellationToken::new(),
        )
        .unwrap();

    assert!(
        analysis.diagnostics().is_empty(),
        "the exact Pop.Ffi dependency and verified .popc metadata must enable the generated callback Module: {:?}",
        analysis.diagnostics()
    );
    let manifest_path = root.join("bubble.toml");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let generator = manifest
        .lines()
        .find(|line| line.starts_with("Example = "))
        .unwrap();
    std::fs::write(
        &manifest_path,
        format!(
            "{manifest}{}\n",
            generator.replacen("Example = ", "Duplicate = ", 1)
        ),
    )
    .unwrap();
    let ambiguous = server
        .change(
            &uri,
            DocumentVersion::new(2),
            active.clone(),
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        ambiguous
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "POP5000"),
        "multiple generators owning one path must not attach either sidecar"
    );
    std::fs::write(&manifest_path, manifest).unwrap();
    std::fs::write(
        root.join("src/generated/example/native-bindings.popc"),
        "stale generated callback metadata",
    )
    .unwrap();
    let stale = server
        .change(
            &uri,
            DocumentVersion::new(3),
            active,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        stale
            .diagnostics()
            .iter()
            .map(pop_language_server::ProtocolDiagnostic::code)
            .collect::<Vec<_>>(),
        vec!["POP5082"],
        "malformed generated metadata must publish only its structured preflight root cause"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ffi_names_remain_unavailable_without_an_exact_direct_pop_ffi_dependency() {
    let mut server = LanguageServer::initialize(Some("en")).expect("server");
    let uri = DocumentUri::new("file:///workspace/unverifiedFfi.pop").expect("URI");
    let analysis = server
        .open(
            uri,
            DocumentVersion::new(1),
            "@Ffi.Foreign(\"value\")\nnamespace Example\ninternal function value(): Ffi.C.Int\nend\n",
            &CancellationToken::new(),
        )
        .expect("analysis");

    assert!(
        analysis
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "POP1002"),
        "reserved FFI names must not become ambient globals"
    );
}

#[test]
fn nearest_nested_package_wins_without_merging_outer_visibility() {
    let root = std::env::temp_dir().join(format!("PopLspNested{}", std::process::id()));
    let inner = root.join("packages/Inner");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(inner.join("src")).unwrap();
    std::fs::write(
        root.join("bubble.toml"),
        "[package]\nname = \"Studio.Outer\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    std::fs::write(
        inner.join("bubble.toml"),
        "[package]\nname = \"Studio.Inner\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let active = "namespace Studio.Inner\nfunction value(): Int\n    return outerOnly()\nend\n";
    std::fs::write(inner.join("src/lib.pop"), active).unwrap();
    let uri = DocumentUri::new(format!("file://{}", inner.join("src/lib.pop").display())).unwrap();
    let mut server = LanguageServer::initialize(Some("en")).unwrap();
    let analysis = server
        .open(
            uri,
            DocumentVersion::new(1),
            active,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        analysis
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "POP1002")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn outer_package_scan_does_not_absorb_nested_package_sources() {
    let root = std::env::temp_dir().join(format!("PopLspOuter{}", std::process::id()));
    let inner = root.join("src/vendor/Inner");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(inner.join("src")).unwrap();
    std::fs::write(
        root.join("bubble.toml"),
        "[package]\nname = \"Studio.Outer\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    std::fs::write(
        inner.join("bubble.toml"),
        "[package]\nname = \"Studio.Inner\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let active = "namespace Studio.Outer\nfunction value(): Int\n    return 1\nend\n";
    std::fs::write(root.join("src/lib.pop"), active).unwrap();
    std::fs::write(
        inner.join("src/lib.pop"),
        "namespace Studio.Outer\nfunction value(): Int\n    return 2\nend\n",
    )
    .unwrap();

    let uri = DocumentUri::new(format!("file://{}", root.join("src/lib.pop").display())).unwrap();
    let mut server = LanguageServer::initialize(Some("en")).unwrap();
    let analysis = server
        .open(
            uri,
            DocumentVersion::new(1),
            active,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        analysis.diagnostics().is_empty(),
        "nested Package sources must not enter the outer Bubble: {:?}",
        analysis.diagnostics()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn document_symbols_never_include_sibling_module_declarations() {
    let root = std::env::temp_dir().join(format!("PopLspSymbols{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("bubble.toml"),
        "[package]\nname = \"Studio.Symbols\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let active = "namespace Studio.Symbols\nfunction active(): Int\n    return 1\nend\n";
    std::fs::write(root.join("src/lib.pop"), active).unwrap();
    std::fs::write(
        root.join("src/sibling.pop"),
        "namespace Studio.Symbols\nfunction siblingWithALongerName(): Int\n    return 2\nend\n",
    )
    .unwrap();

    let uri = DocumentUri::new(format!("file://{}", root.join("src/lib.pop").display())).unwrap();
    let mut server = LanguageServer::initialize(Some("en")).unwrap();
    server
        .open(
            uri.clone(),
            DocumentVersion::new(1),
            active,
            &CancellationToken::new(),
        )
        .unwrap();
    let symbols = server
        .document_symbols(&uri, &CancellationToken::new())
        .unwrap();
    assert_eq!(
        symbols
            .iter()
            .map(pop_language_server::DocumentSymbol::name)
            .collect::<Vec<_>>(),
        ["active"]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn closing_a_deleted_module_reanalyzes_its_previous_bubble() {
    let root = std::env::temp_dir().join(format!("PopLspDeleted{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("bubble.toml"),
        "[package]\nname = \"Studio.Deleted\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let library = "namespace Studio.Deleted\nfunction value(): Int\n    return helper()\nend\n";
    let helper = "namespace Studio.Deleted\nfunction helper(): Int\n    return 1\nend\n";
    std::fs::write(root.join("src/lib.pop"), library).unwrap();
    std::fs::write(root.join("src/helper.pop"), helper).unwrap();
    let library_uri =
        DocumentUri::new(format!("file://{}", root.join("src/lib.pop").display())).unwrap();
    let helper_uri =
        DocumentUri::new(format!("file://{}", root.join("src/helper.pop").display())).unwrap();
    let mut server = LanguageServer::initialize(Some("en")).unwrap();
    server
        .open(
            library_uri.clone(),
            DocumentVersion::new(1),
            library,
            &CancellationToken::new(),
        )
        .unwrap();
    server
        .open(
            helper_uri.clone(),
            DocumentVersion::new(1),
            helper,
            &CancellationToken::new(),
        )
        .unwrap();

    std::fs::remove_file(root.join("src/helper.pop")).unwrap();
    assert!(server.close(&helper_uri));

    let analysis = server
        .analyze(&library_uri, &CancellationToken::new())
        .unwrap();
    assert!(
        analysis
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "POP1002"),
        "the remaining Module must be reanalyzed without the deleted helper"
    );
    std::fs::remove_dir_all(root).unwrap();
}
