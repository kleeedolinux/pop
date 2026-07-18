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
    assert!(
        analysis
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "POP6401")
    );
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
            uri,
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
