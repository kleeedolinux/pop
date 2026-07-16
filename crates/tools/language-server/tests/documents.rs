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
    assert_eq!(
        first.diagnostics().len(),
        0,
        "syntax-only slice has valid syntax"
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
