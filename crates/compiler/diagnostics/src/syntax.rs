use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticSeverity,
    FixApplicability, MessageKey, QuickFix, SourceSpan, TextEdit, WorkspaceEdit,
};

#[must_use]
pub fn unexpected_token(
    span: SourceSpan,
    expectation: &'static str,
    found: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP0002"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.unexpectedToken"),
        vec![
            DiagnosticArgument::SyntaxExpectation(expectation),
            DiagnosticArgument::Token(found.into()),
        ],
        span,
    )
}

#[must_use]
pub fn misspelled_return(span: SourceSpan, found: impl Into<String>) -> Diagnostic {
    let edit = TextEdit::new(span.file(), span.range(), "return");
    let fix = QuickFix::new(
        "replaceMisspelledReturn",
        MessageKey::new("fix.replaceMisspelledReturn"),
        FixApplicability::Safe,
        WorkspaceEdit::new(0, vec![edit]),
    )
    .with_fix_all_equivalence("replaceMisspelledReturn");
    unexpected_token(span, "`return`", found).with_fix(fix)
}

#[must_use]
pub fn missing_namespace(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP0003"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.missingNamespace"),
        Vec::new(),
        span,
    )
}

#[must_use]
pub fn unsupported_export(span: SourceSpan) -> Diagnostic {
    let edit = TextEdit::new(span.file(), span.range(), "public");
    let fix = QuickFix::new(
        "replaceExportWithPublic",
        MessageKey::new("fix.replaceExportWithPublic"),
        FixApplicability::Safe,
        WorkspaceEdit::new(0, vec![edit]),
    )
    .with_fix_all_equivalence("replaceExportWithPublic");
    Diagnostic::new(
        DiagnosticCode::new("POP0004"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.unsupportedExport"),
        vec![DiagnosticArgument::Identifier("export".to_owned())],
        span,
    )
    .with_fix(fix)
}
